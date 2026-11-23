//! Clipboard capture.
//!
//! Capture is poll-driven off `GetClipboardSequenceNumber()` rather than a
//! `WM_CLIPBOARDUPDATE` listener. The sequence number is a single cheap call that
//! changes only when the clipboard changes, so a 350 ms tick costs effectively
//! nothing, and it avoids a second message-only window competing with Tauri's
//! event loop.
//!
//! Text is read and written through the raw Win32 API instead of a helper crate
//! so the exact bytes survive a round trip: CRLF, lone surrogates, and embedded
//! NUL handling all matter for a tool whose entire job is reproducing what the
//! user copied.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::animated;
use crate::error::{AppError, Result};
use crate::hashing;
#[cfg(target_os = "macos")]
use crate::macos_clipboard;
use crate::models::{ClipContent, ClipKind, NewClip};
use crate::search;
use crate::settings::Settings;
use crate::store::Store;

/// Characters kept for the list preview.
const PREVIEW_CHARS: usize = 320;

pub const EVENT_HISTORY_CHANGED: &str = "history:changed";
/// Emitted whenever the private capture pause starts, changes or ends.
pub const EVENT_CAPTURE_PAUSED: &str = "capture:paused";
/// Emitted when a copy has scheduled the clipboard to be emptied.
pub const EVENT_CLIPBOARD_CLEAR_SCHEDULED: &str = "clipboard:clear-scheduled";

/// Why capture is currently not recording, or that it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PauseState {
    pub paused: bool,
    /// Unix-millisecond deadline of a timed pause.
    pub until: Option<i64>,
    /// True while paused "until restart".
    pub indefinite: bool,
}

// ---------------------------------------------------------------------------
// Watcher
// ---------------------------------------------------------------------------

/// Owns the background capture thread and the "ignore my own write" latch.
pub struct Watcher {
    /// Content hash of something this app just put on the clipboard. The next
    /// matching capture is dropped so selecting an item does not immediately
    /// re-record it and reshuffle the list under the user's cursor.
    suppress: Arc<Mutex<Option<String>>>,
    running: Arc<AtomicBool>,
    /// Private-mode pause: a timed deadline, or the "until restart" flag. While
    /// paused the sequence counter still advances so nothing copied in the
    /// meantime is captured on resume.
    paused_until: Arc<Mutex<Option<Instant>>>,
    paused_forever: Arc<AtomicBool>,
}

impl Watcher {
    pub fn new() -> Self {
        Watcher {
            suppress: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicBool::new(false)),
            paused_until: Arc::new(Mutex::new(None)),
            paused_forever: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn suppress_hash(&self, hash: String) {
        *self.suppress.lock() = Some(hash);
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }

    /// Stops capture for `duration`.
    pub fn pause_for(&self, duration: Duration) {
        self.paused_forever.store(false, Ordering::SeqCst);
        *self.paused_until.lock() = Some(Instant::now() + duration);
    }

    /// Stops capture until the application is restarted.
    pub fn pause_until_restart(&self) {
        *self.paused_until.lock() = None;
        self.paused_forever.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused_forever.store(false, Ordering::SeqCst);
        *self.paused_until.lock() = None;
    }

    pub fn pause_state(&self) -> PauseState {
        if self.paused_forever.load(Ordering::SeqCst) {
            return PauseState {
                paused: true,
                until: None,
                indefinite: true,
            };
        }
        match *self.paused_until.lock() {
            Some(deadline) if deadline > Instant::now() => PauseState {
                paused: true,
                until: Some(millis_until(deadline)),
                indefinite: false,
            },
            _ => PauseState {
                paused: false,
                until: None,
                indefinite: false,
            },
        }
    }

    pub fn start(&self, app: AppHandle, store: Arc<Store>) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let suppress = Arc::clone(&self.suppress);
        let running = Arc::clone(&self.running);
        let paused_until = Arc::clone(&self.paused_until);
        let paused_forever = Arc::clone(&self.paused_forever);

        let spawned = std::thread::Builder::new()
            .name("clipboard-watcher".into())
            .spawn(move || {
                let mut last = sequence_number();
                while running.load(Ordering::Relaxed) {
                    let settings = store.settings();
                    std::thread::sleep(Duration::from_millis(settings.poll_interval_ms));
                    if !running.load(Ordering::Relaxed) {
                        break;
                    }

                    let current = sequence_number();
                    if current == last {
                        continue;
                    }
                    last = current;

                    // A timed pause that has just run out is noticed here, so the
                    // indicator in the UI clears itself even if nothing is copied.
                    let expired = {
                        let mut deadline = paused_until.lock();
                        let expired = deadline.is_some_and(|at| Instant::now() >= at);
                        if expired {
                            *deadline = None;
                        }
                        expired
                    };
                    if expired {
                        let _ = app.emit(EVENT_CAPTURE_PAUSED, PauseState {
                            paused: false,
                            until: None,
                            indefinite: false,
                        });
                    }
                    if paused_forever.load(Ordering::SeqCst) || paused_until.lock().is_some() {
                        log::debug!("capture is paused; ignoring a clipboard change");
                        continue;
                    }

                    // A panic while interpreting hostile clipboard data must not
                    // silently disable capture for the rest of the session.
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        capture_once(&app, &store, &suppress, &settings)
                    }));
                    match outcome {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => log::warn!("clipboard capture failed: {e}"),
                        Err(_) => log::error!("clipboard capture panicked; continuing"),
                    }
                }
                log::info!("clipboard watcher stopped");
            });

        if let Err(e) = spawned {
            self.running.store(false, Ordering::SeqCst);
            log::error!("could not start the clipboard watcher thread: {e}");
        }
    }
}

/// Converts an `Instant` deadline into a wall-clock timestamp the UI can render.
fn millis_until(deadline: Instant) -> i64 {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    now + remaining.as_millis() as i64
}

impl Default for Watcher {
    fn default() -> Self {
        Watcher::new()
    }
}

/// Answers whether `hash` is the write this app has just performed, and empties the
/// latch when it is.
fn swallow_own_write(suppress: &Arc<Mutex<Option<String>>>, hash: &str) -> bool {
    let mut latch = suppress.lock();
    if latch.as_deref() != Some(hash) {
        return false;
    }
    *latch = None;
    true
}

fn capture_once(
    app: &AppHandle,
    store: &Arc<Store>,
    suppress: &Arc<Mutex<Option<String>>>,
    settings: &Settings,
) -> Result<()> {    // Attribute the clipboard to whatever was in front when the change landed.
    let foreground = foreground_process_name();

    if let Some(name) = &foreground
        && settings
            .ignored_apps
            .iter()
            .any(|ignored| ignored.eq_ignore_ascii_case(name))
    {
        log::debug!("ignoring a copy from {name}");
        return Ok(());
    }

    // Copies made while our own window is focused are still recorded: the hash
    // latch is what prevents feedback loops, and dropping them here would silently
    // lose anything copied out of the preview pane. They are merely not
    // *attributed* to us, which would be meaningless in the UI.
    let source_app = foreground.filter(|name| !is_self(name));

    let Some(content) = read_content(settings) else {
        return Ok(());
    };

    let hash = hashing::of_content(&content);
    if swallow_own_write(suppress, &hash) {
        // This is a write we just performed; nothing to record.
        return Ok(());
    }

    let (preview, search_text) = describe(&content);
    let kind = content.kind();
    // A browser's "Copy image" on a GIF or video publishes only the frame it is
    // showing, but its CF_HTML still names the source asset. Keep that candidate
    // so the moving version can be fetched while the still frame already serves
    // the list. A pasted address takes the same route.
    let motion_source = match kind {
        ClipKind::Image => motion_html().as_deref().and_then(animated::find),
        ClipKind::Text => match &content {
            ClipContent::Text(text) => animated::from_url(text),
            _ => None,
        },
        ClipKind::Files => None,
    };
    // Secrets can be kept out of history entirely; the flag itself is derived in
    // `insert_clip` so every capture path shares one rule set.
    if kind == ClipKind::Text && settings.skip_secret_capture {
        if let ClipContent::Text(text) = &content {
            if crate::secrets::detect(text, settings.secret_heuristics).is_some() {
                log::debug!("left a likely secret out of history");
                return Ok(());
            }
        }
    }
    // Rich flavours ride along with the plain text when the source offered them,
    // so a styled copy can be pasted back with its formatting intact.
    let (html, rtf) = if kind == ClipKind::Text && settings.paste_rich_text {
        read_rich_text()
    } else {
        (None, None)
    };
    // The optional automatic preview needs the address before `content` is moved
    // into the row it belongs to.
    let preview_link = if settings.link_previews_auto && kind == ClipKind::Text {
        match &content {
            ClipContent::Text(text) => {
                let url = text.trim();
                (crate::color::classify(url) == Some(crate::color::ContentKind::Url)
                    && crate::preview::is_fetchable(url))
                .then(|| url.to_string())
            }
            _ => None,
        }
    } else {
        None
    };
    let new = NewClip {
        content,
        content_hash: hash,
        source_app,
        search_text,
        preview,
        html,
        rtf,
    };

    let (id, inserted) = match store.insert_clip(new)? {
        crate::store::InsertOutcome::Inserted(id) => {
            log::debug!("captured a new clip");
            (id, true)
        }
        crate::store::InsertOutcome::Duplicate(id) => {
            log::debug!("promoted an existing clip");
            (id, false)
        }
    };

    if kind == ClipKind::Image && inserted {
        schedule_ocr(app, Arc::clone(store), id);
        schedule_faces(app, Arc::clone(store), id);
    }
    // Optional feedback for every capture, not just the copies made from inside the
    // app. Off by default: a tick on every Ctrl+C in every program is a lot to live
    // with. Only a genuinely new clip makes the noise — copying something already in
    // the history promotes the old row and has nothing new to announce.
    if inserted && settings.sound_on_capture {
        crate::sound::play(settings, crate::sound::Outcome::Copied);
    }
    // Both paths end here: a frame whose page named an asset, and a pasted link.
    if let Some(source) = motion_source {
        schedule_motion(app, Arc::clone(store), id, source);
    }
    // Link previews are opt-in. Without the setting nothing about a copied address
    // leaves the machine, which is the promise the rest of the app keeps.
    if let Some(url) = preview_link {
        let has_preview = store
            .detail(id)
            .map(|detail| detail.summary.thumb_file.is_some())
            .unwrap_or(false);
        if !has_preview {
            schedule_link_preview(app, Arc::clone(store), id, url);
        }
    }

    if settings.max_items > 0 && store.total() > settings.max_items as usize {
        let pruned = store.prune()?;
        if pruned > 0 {
            log::debug!("pruned {pruned} clip(s) past the retention cap");
        }
    }

    let _ = app.emit(EVENT_HISTORY_CHANGED, ());
    Ok(())
}

/// Recognises an image's text off the capture thread.
///
/// Capture must stay responsive — the watcher thread is the only thing reading the
/// clipboard — so the expensive part happens here, and the UI is told to re-read
/// once the index has changed. Shared with screenshots, which capture an image the
/// same way.
pub(crate) fn schedule_ocr(app: &AppHandle, store: Arc<Store>, id: i64) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("clipboard-ocr".into())
        .spawn(move || {
            if let Err(e) = store.run_ocr(id) {
                log::warn!("text recognition failed for clip {id}: {e}");
            }
            let _ = app.emit(EVENT_HISTORY_CHANGED, ());
        });
    if let Err(e) = spawned {
        log::error!("could not start the text recognition thread: {e}");
    }
}

/// Looks for faces in an image clip off the capture thread, so `Images → People`
/// knows about it. Skipped entirely when the setting is off.
pub(crate) fn schedule_faces(app: &AppHandle, store: Arc<Store>, id: i64) {
    if !store.settings().detect_faces {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("clipboard-faces".into())
        .spawn(move || {
            if let Err(e) = store.run_faces(id) {
                log::warn!("face detection failed for clip {id}: {e}");
            }
            let _ = app.emit(EVENT_HISTORY_CHANGED, ());
        });
    if let Err(e) = spawned {
        log::error!("could not start the face detection thread: {e}");
    }
}

/// Fetches the moving asset behind an image copy without delaying capture.
///
/// Runs on the async runtime because capture's watcher thread must get back to
/// reading the clipboard. Any failure is a quiet fallback to the still frame.
///
/// The clip is flagged as pending before the request and un-flagged after it, and
/// the list is told both times: the fetch can take seconds, and a card that stays
/// a plain link until it lands gives no sign that anything is happening.
fn schedule_motion(
    app: &AppHandle,
    store: Arc<Store>,
    id: i64,
    source: animated::MotionSource,
) {
    if store.motion(id).is_some() {
        return;
    }
    store.set_motion_pending(id, true);
    announce(app, &store, id);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let kind = source.kind();
        // Remembered before the download so a slow or failed fetch still leaves the
        // address available to "Copy Video URL".
        if kind.is_video()
            && let animated::MotionSource::Remote { url, .. } = &source
            && let Err(e) = store.set_motion_url(id, url)
        {
            log::debug!("could not record the video address for clip {id}: {e}");
        }
        let stored = match animated::load(&source).await {
            Ok(bytes) => store.attach_motion(id, kind, &bytes),
            Err(e) => {
                log::debug!("kept the static frame for clip {id}: {e}");
                Ok(())
            }
        };
        if let Err(e) = &stored {
            log::debug!("could not store the media for clip {id}: {e}");
        } else if store.motion(id).is_some() {
            log::info!("kept the .{} media for clip {id}", kind.extension());
        }
        // Cleared either way, so a failed fetch does not leave a spinner behind.
        store.set_motion_pending(id, false);
        announce(&app, &store, id);
    });
}

/// Rebuilds one clip and tells the window, so a pending or finished fetch shows.
fn announce(app: &AppHandle, store: &Arc<Store>, id: i64) {
    if let Err(e) = store.refresh(id) {
        log::debug!("could not refresh clip {id}: {e}");
    }
    let _ = app.emit(EVENT_HISTORY_CHANGED, ());
}

/// Fetches a link's preview picture off the capture thread.
///
/// Only ever called when the user has turned automatic previews on; a failure is
/// quiet, because a page without an image is not an error the user asked about.
fn schedule_link_preview(app: &AppHandle, store: Arc<Store>, id: i64, url: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match crate::preview::store_for(&store, id, &url).await {
            Ok(_) => {
                log::info!("kept a link preview for clip {id}");
                let _ = app.emit(EVENT_HISTORY_CHANGED, ());
            }
            Err(e) => log::debug!("no link preview for clip {id}: {e}"),
        }
    });
}

/// Builds the preview text and the search mirror for a captured payload.
fn describe(content: &ClipContent) -> (String, String) {
    match content {
        ClipContent::Text(text) => {
            let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let preview = collapsed.chars().take(PREVIEW_CHARS).collect();
            (preview, search::normalize(text))
        }
        ClipContent::Files(paths) => {
            let names: Vec<String> = paths.iter().map(|p| leaf_name(p)).collect();
            let preview = if names.len() == 1 {
                names[0].clone()
            } else {
                let shown: Vec<&str> = names.iter().take(4).map(String::as_str).collect();
                format!("{} files · {}", names.len(), shown.join(", "))
            };
            (preview, search::normalize(&paths.join(" ")))
        }
        ClipContent::Image { width, height, .. } => {
            (format!("Image {width} × {height}"), String::new())
        }
    }
}

fn leaf_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

fn is_self(process_name: &str) -> bool {
    std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(|path| path.file_name().map(|n| n.to_string_lossy().to_string()))
        .is_some_and(|own| own.eq_ignore_ascii_case(process_name))
}

/// Reads whichever representation the clipboard currently offers, honouring the
/// capture preferences.
fn read_content(settings: &Settings) -> Option<ClipContent> {
    if settings.ignore_sensitive && clipboard_opt_out() {
        log::debug!("clipboard owner asked to be excluded from history");
        return None;
    }

    if settings.capture_files
        && let Some(paths) = read_file_list()
        && !paths.is_empty()
    {
        return Some(ClipContent::Files(paths));
    }

    let text = read_text();
    let image_first = settings.capture_images && settings.prefer_images_over_text;
    if image_first && let Some(image) = read_image() {
        return Some(image);
    }

    if let Some(text) = text.filter(|t| !t.is_empty()) {
        return Some(ClipContent::Text(text));
    }

    if settings.capture_images
        && !image_first
        && let Some(image) = read_image()
    {
        return Some(image);
    }

    None
}

/// The HTML a copy carries, which is where an animated image's address hides.
///
/// "Copy image" in a browser publishes only the frame on screen plus an `<img>`
/// tag naming the asset it came from, so the moving version is findable only
/// through that HTML.
#[cfg(windows)]
fn motion_html() -> Option<String> {
    read_rich_text().0
}

/// The same half of the copy taken with `xclip`.
///
/// `arboard` carries no HTML, and X11 spells the target `text/html` rather than
/// the registered `HTML Format`, so it is read through the tool that already
/// publishes file lists.
#[cfg(target_os = "linux")]
fn motion_html() -> Option<String> {
    let output = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "text/html", "-o"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let html = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!html.is_empty()).then_some(html)
}

/// The HTML half of a motion copy on macOS: the same `public.html` a browser
/// writes beside the file.
#[cfg(target_os = "macos")]
fn motion_html() -> Option<String> {
    macos_clipboard::motion_html()
}

// ---------------------------------------------------------------------------
// Public clipboard write
// ---------------------------------------------------------------------------

/// Places `content` on the clipboard. Returns the content hash so the caller can
/// arm the suppression latch.
pub fn write(content: &ClipContent) -> Result<String> {
    write_flavoured(content, None, None)
}

/// Writes `content` and, for text, the rich flavours captured with it.
#[cfg(windows)]
pub fn write_flavoured(
    content: &ClipContent,
    html: Option<&str>,
    rtf: Option<&str>,
) -> Result<String> {
    match content {
        // A picture keeps the same retrying clipboard session as everything else.
        // arboard's own session gives up the first time another process is holding
        // the clipboard open, which is what made images fail to paste while text
        // from the same list pasted fine.
        ClipContent::Image {
            width,
            height,
            rgba,
        } => set_image(*width, *height, rgba)?,
        _ => {
            let outcome = with_clipboard(|| -> Result<()> {
                unsafe { EmptyClipboard()? };
                match content {
                    ClipContent::Text(text) => {
                        set_text(text)?;
                        if let Some(html) = html.filter(|value| !value.trim().is_empty()) {
                            set_html(html)?;
                        }
                        if let Some(rtf) = rtf.filter(|value| !value.trim().is_empty()) {
                            set_rtf(rtf)?;
                        }
                    }
                    ClipContent::Files(paths) => set_files(paths)?,
                    ClipContent::Image { .. } => unreachable!("handled above"),
                }
                Ok(())
            });
            match outcome {
                Some(result) => result?,
                None => {
                    return Err(AppError::Clipboard(
                        "another process is holding the clipboard open".into(),
                    ));
                }
            }
        }
    }
    Ok(hashing::of_content(content))
}

/// Places a downloaded file on the clipboard alongside its source URL.
///
/// Image-aware targets (chat apps, editors) paste the animated GIF or video from
/// the file drop, while plain-text targets get the link. Kept separate from
/// [`write_flavoured`] because it publishes two formats in one clipboard session.
/// X11 has no clipboard session to hold open and arboard carries no rich
/// flavours, so this writes what the port supports: text, an image, or a file
/// list as its paths.
#[cfg(target_os = "linux")]
pub fn write_flavoured(
    content: &ClipContent,
    html: Option<&str>,
    rtf: Option<&str>,
) -> Result<String> {
    let _ = (html, rtf);
    // A file list is published by `xclip`, which owns the selection itself.
    if let ClipContent::Files(paths) = content {
        return write_files(paths);
    }
    with_owner(|clip| match content {
        ClipContent::Text(text) => clip
            .set_text(text.clone())
            .map_err(|e| AppError::Clipboard(e.to_string())),
        ClipContent::Image {
            width,
            height,
            rgba,
        } => clip
            .set_image(arboard::ImageData {
                width: *width as usize,
                height: *height as usize,
                bytes: std::borrow::Cow::Borrowed(rgba.as_slice()),
            })
            .map_err(|e| AppError::Clipboard(e.to_string())),
        ClipContent::Files(_) => unreachable!("handled above"),
    })?;
    Ok(hashing::of_content(content))
}

/// The clipboard context that owns what Kept copied, for the life of the process.
///
/// X11 has no clipboard daemon by default: a selection belongs to the process that
/// set it until that connection goes away. Dropping an `arboard::Clipboard` right
/// after `set_text` therefore releases the selection, and the clipboard reads as
/// empty a moment later — which is what made "copy, then paste" paste nothing and
/// made the clipboard warn that no manager took the contents over.
#[cfg(target_os = "linux")]
static OWNER: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

#[cfg(target_os = "linux")]
fn with_owner<T>(write: impl FnOnce(&mut arboard::Clipboard) -> Result<T>) -> Result<T> {
    let mut slot = OWNER.lock();
    if slot.is_none() {
        *slot = Some(
            arboard::Clipboard::new()
                .map_err(|e| AppError::Clipboard(format!("could not open the clipboard: {e}")))?,
        );
    }
    let clip = slot.as_mut().expect("filled just above");
    write(clip)
}

#[cfg(windows)]
pub fn write_file_and_text(path: &Path, text: &str) -> Result<String> {
    let paths = vec![path.to_string_lossy().into_owned()];
    let outcome = with_clipboard(|| -> Result<()> {
        unsafe { EmptyClipboard()? };
        set_files(&paths)?;
        set_text(text)?;
        Ok(())
    });
    match outcome {
        Some(result) => result?,
        None => {
            return Err(AppError::Clipboard(
                "another process is holding the clipboard open".into(),
            ));
        }
    }
    Ok(hashing::of_content(&ClipContent::Files(paths)))
}

#[cfg(target_os = "linux")]
pub fn write_file_and_text(path: &Path, text: &str) -> Result<String> {
    let paths = vec![path.to_string_lossy().into_owned()];
    // One process owns the X11 selection at a time, so the file list and the
    // source link cannot both be published: the file is the useful half.
    let _ = text;
    write_files(&paths)
}

/// Publishes one or more paths as `CF_HDROP` only.
///
/// Used when an image clip has a recovered animation: GIF-aware targets paste the
/// file and keep every frame, while the still frame stays the list preview.
#[cfg(windows)]
pub fn write_files(paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Err(AppError::Clipboard("there is no file to copy".into()));
    }
    let outcome = with_clipboard(|| -> Result<()> {
        unsafe { EmptyClipboard()? };
        set_files(paths)?;
        Ok(())
    });
    match outcome {
        Some(result) => result?,
        None => {
            return Err(AppError::Clipboard(
                "another process is holding the clipboard open".into(),
            ));
        }
    }
    Ok(hashing::of_content(&ClipContent::Files(paths.to_vec())))
}

/// Publishes the paths as an X11 file list, which is what file managers and chat
/// apps paste as files.
#[cfg(target_os = "linux")]
pub fn write_files(paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Err(AppError::Clipboard("there is no file to copy".into()));
    }
    let listing: String = paths
        .iter()
        .map(|path| format!("{}\r\n", file_uri(path)))
        .collect();
    set_selection("text/uri-list", &listing)?;
    Ok(hashing::of_content(&ClipContent::Files(paths.to_vec())))
}

/// Hands one target to `xclip`, which then serves the selection for as long as
/// anything wants it — the X11 stand-in for keeping a clipboard session open.
#[cfg(target_os = "linux")]
fn set_selection(target: &str, contents: &str) -> Result<()> {
    use std::io::Write as _;

    // Both ends are discarded: `xclip` forks into the background to serve the
    // selection, and a forked child holding our pipes would keep them open, so a
    // read of them would never see the end of the data.
    let mut child = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-t", target, "-i"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| AppError::Clipboard(format!("xclip is not available ({e}); install xclip")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(contents.as_bytes())
            .map_err(|e| AppError::Clipboard(format!("xclip took no data: {e}")))?;
    }
    // The process that read the data is the one that exits; the copy it left
    // behind is what answers the next paste.
    match child.wait() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(AppError::Clipboard(format!(
            "xclip could not publish the file list ({status})"
        ))),
        Err(e) => Err(AppError::Clipboard(format!("xclip did not finish: {e}"))),
    }
}

/// A local path spelled as the `file://` URI a `text/uri-list` carries.
#[cfg(target_os = "linux")]
fn file_uri(path: &str) -> String {
    let mut uri = String::from("file://");
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                uri.push(byte as char);
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// The local paths a `text/uri-list` names, ignoring blank lines and comments.
#[cfg(target_os = "linux")]
fn parse_uri_list(listing: &str) -> Vec<String> {
    listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("file://"))
        .map(percent_decode)
        .filter(|path| !path.is_empty())
        .collect()
}

/// Undoes the `%XX` escapes in a URI, leaving anything unescaped as written.
#[cfg(not(windows))]
pub(crate) fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(value) = u8::from_str_radix(&text[index + 1..index + 3], 16)
        {
            out.push(value);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Empties the clipboard, used by the auto-clear timer.
pub fn clear() -> Result<()> {
    #[cfg(windows)]
    {
        let outcome = with_clipboard(|| unsafe { EmptyClipboard() });
        match outcome {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(AppError::Clipboard(format!(
                "could not empty the clipboard: {e}"
            ))),
            None => Err(AppError::Clipboard(
                "another process is holding the clipboard open".into(),
            )),
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Cocoa does have an empty pasteboard, so the Mac clears it properly
        // rather than leaving an empty string behind.
        if macos_clipboard::write_text("", None, None) {
            Ok(())
        } else {
            Err(AppError::Clipboard("could not empty the clipboard".into()))
        }
    }
    #[cfg(target_os = "linux")]
    {
        // X11 cannot be emptied the way `EmptyClipboard` does it: a selection has
        // to be owned by someone. An empty string is the closest thing to "there is
        // nothing to paste", and the owner Kept already holds can serve it.
        with_owner(|clip| {
            clip.set_text(String::new())
                .map_err(|e| AppError::Clipboard(format!("could not clear the clipboard: {e}")))
        })
    }
}

// ---------------------------------------------------------------------------
// Windows implementation
// ---------------------------------------------------------------------------

#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, GlobalFree, HANDLE, HGLOBAL, HWND};
#[cfg(windows)]
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
#[cfg(windows)]
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
#[cfg(windows)]
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
#[cfg(windows)]
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
#[cfg(windows)]
use windows::core::PWSTR;

#[cfg(windows)]
const CF_UNICODETEXT: u32 = 13;
/// Device-independent bitmap: a header followed by the pixels.
#[cfg(windows)]
const CF_DIB: u32 = 8;
/// `CF_HDROP`; defined locally because the windows crate nests it under Ole.
#[cfg(windows)]
const CF_HDROP: u32 = 15;

#[cfg(windows)]
pub fn sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// Runs `f` with the clipboard open, retrying while another process holds it.
///
/// Returns `None` if the clipboard stayed locked. `f` must not panic: the paired
/// `CloseClipboard` lives after the call.
#[cfg(windows)]
fn with_clipboard<T>(f: impl FnOnce() -> T) -> Option<T> {
    for attempt in 0..8u64 {
        // SAFETY: a successful OpenClipboard is always matched by CloseClipboard.
        if unsafe { OpenClipboard(None) }.is_ok() {
            let value = f();
            let _ = unsafe { CloseClipboard() };
            return Some(value);
        }
        std::thread::sleep(Duration::from_millis(10 * (attempt + 1)));
    }
    None
}

/// Reads the `HGLOBAL` behind a clipboard format as a byte slice.
///
/// # Safety
/// Must be called with the clipboard open.
#[cfg(windows)]
unsafe fn hglobal_bytes(handle: HANDLE) -> Option<(*mut u8, usize)> {
    let hglobal = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(hglobal) };
    if size == 0 {
        return None;
    }
    let pointer = unsafe { GlobalLock(hglobal) }.cast::<u8>();
    if pointer.is_null() {
        return None;
    }
    Some((pointer, size))
}

/// # Safety
/// Must be called with the clipboard open.
#[cfg(windows)]
unsafe fn unlock(handle: HANDLE) {
    let _ = unsafe { GlobalUnlock(HGLOBAL(handle.0)) };
}

#[cfg(windows)]
fn read_text() -> Option<String> {
    with_clipboard(|| unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
        let (pointer, size) = hglobal_bytes(handle)?;
        let mut units = Vec::with_capacity(size / 2);
        let capacity = size / 2;
        for index in 0..capacity {
            let unit = *pointer.cast::<u16>().add(index);
            if unit == 0 {
                break;
            }
            units.push(unit);
        }
        unlock(handle);
        if units.is_empty() {
            None
        } else {
            Some(String::from_utf16_lossy(&units))
        }
    })
    .flatten()
}

#[cfg(windows)]
fn read_file_list() -> Option<Vec<String>> {
    with_clipboard(|| unsafe {
        if IsClipboardFormatAvailable(CF_HDROP).is_err() {
            return None;
        }
        let handle = GetClipboardData(CF_HDROP).ok()?;
        let drop = HDROP(handle.0);
        // 0xFFFF_FFFF asks DragQueryFileW for the item count.
        let count = DragQueryFileW(drop, u32::MAX, None);
        if count == 0 {
            return None;
        }
        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            let length = DragQueryFileW(drop, index, None);
            if length == 0 {
                continue;
            }
            let mut buffer = vec![0u16; length as usize + 1];
            let written = DragQueryFileW(drop, index, Some(&mut buffer));
            if written == 0 {
                continue;
            }
            buffer.truncate(written as usize);
            paths.push(String::from_utf16_lossy(&buffer));
        }
        (!paths.is_empty()).then_some(paths)
    })
    .flatten()
}

/// Reads both rich flavours in one clipboard session, so the HTML and RTF come
/// from the same copy.
#[cfg(windows)]
fn read_rich_text() -> (Option<String>, Option<String>) {
    let html = read_registered_bytes("HTML Format").and_then(|bytes| cf_html_fragment(&bytes));
    let rtf = read_registered_bytes("Rich Text Format")
        .map(|bytes| String::from_utf8_lossy(&bytes).trim_end_matches('\0').to_string())
        .filter(|text| !text.trim().is_empty());
    (html, rtf)
}

/// Snapshot of a named registered clipboard format.
#[cfg(windows)]
fn read_registered_bytes(name: &str) -> Option<Vec<u8>> {
    let units = widened(name);
    let format = unsafe { RegisterClipboardFormatW(windows::core::PCWSTR(units.as_ptr())) };
    if format == 0 {
        return None;
    }
    with_clipboard(|| unsafe {
        if IsClipboardFormatAvailable(format).is_err() {
            return None;
        }
        let handle = GetClipboardData(format).ok()?;
        let (pointer, size) = hglobal_bytes(handle)?;
        let bytes = std::slice::from_raw_parts(pointer, size).to_vec();
        unlock(handle);
        Some(bytes)
    })
    .flatten()
}

/// Cuts the fragment out of a `CF_HTML` payload using its own byte offsets.
fn cf_html_fragment(bytes: &[u8]) -> Option<String> {
    let header_len = bytes.len().min(1024);
    let header = String::from_utf8_lossy(&bytes[..header_len]);
    let offset = |key: &str| -> Option<usize> {
        let index = header.find(key)?;
        let rest = header[index + key.len()..].trim_start();
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };

    match (offset("StartFragment:"), offset("EndFragment:")) {
        (Some(start), Some(end)) if start < end && end <= bytes.len() => {
            trim_fragment(&String::from_utf8_lossy(&bytes[start..end]))
        }
        _ => {
            // Malformed or missing offsets: fall back to the document body.
            let text = String::from_utf8_lossy(bytes);
            let lower = text.to_lowercase();
            let start = lower.find("<body").or_else(|| lower.find("<html"))?;
            trim_fragment(&text[start..])
        }
    }
}

fn trim_fragment(text: &str) -> Option<String> {
    let trimmed = text.trim().trim_matches('\0').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(windows)]
fn read_image() -> Option<ClipContent> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let image = clipboard.get_image().ok()?;
    if image.width == 0 || image.height == 0 {
        return None;
    }
    let expected = image.width.checked_mul(image.height)?.checked_mul(4)?;
    if image.bytes.len() < expected {
        log::warn!(
            "clipboard image is {} bytes but {}x{} RGBA needs {expected}",
            image.bytes.len(),
            image.width,
            image.height
        );
        return None;
    }
    Some(ClipContent::Image {
        width: image.width as u32,
        height: image.height as u32,
        rgba: image.bytes[..expected].to_vec(),
    })
}

#[cfg(windows)]
fn widened(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Checks the opt-out formats that Windows clipboard history itself honours.
/// Password managers publish these so their contents never reach a log.
#[cfg(windows)]
fn clipboard_opt_out() -> bool {
    with_clipboard(|| unsafe {
        // Presence of this format is itself the signal, regardless of its value.
        let exclude = RegisterClipboardFormatW(windows::core::PCWSTR(
            widened("ExcludeClipboardContentFromMonitorProcessing").as_ptr(),
        ));
        if exclude != 0 && IsClipboardFormatAvailable(exclude).is_ok() {
            return true;
        }

        for name in ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"] {
            let format = RegisterClipboardFormatW(windows::core::PCWSTR(widened(name).as_ptr()));
            if format == 0 || IsClipboardFormatAvailable(format).is_err() {
                continue;
            }
            let Ok(handle) = GetClipboardData(format) else {
                continue;
            };
            let Some((pointer, size)) = hglobal_bytes(handle) else {
                continue;
            };
            if size >= std::mem::size_of::<u32>() {
                let flag = std::ptr::read_unaligned(pointer.cast::<u32>());
                unlock(handle);
                if flag == 0 {
                    return true;
                }
            } else {
                unlock(handle);
            }
        }
        false
    })
    .unwrap_or(false)
}

#[cfg(windows)]
fn set_text(text: &str) -> Result<()> {
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = units.len() * size_of::<u16>();
    unsafe {
        let hglobal = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| {
            AppError::Clipboard(format!("could not allocate clipboard memory: {e}"))
        })?;
        let destination = GlobalLock(hglobal);
        if destination.is_null() {
            let _ = GlobalFree(Some(hglobal));
            return Err(AppError::Clipboard(
                "could not lock clipboard memory".into(),
            ));
        }
        std::ptr::copy_nonoverlapping(units.as_ptr().cast::<u8>(), destination.cast::<u8>(), bytes);
        let _ = GlobalUnlock(hglobal);

        // On success the system takes ownership of `hglobal`; freeing it here
        // would leave the clipboard pointing at released memory.
        if let Err(e) = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hglobal.0))) {
            let _ = GlobalFree(Some(hglobal));
            return Err(AppError::Clipboard(format!("could not set text: {e}")));
        }
    }
    Ok(())
}

/// Copies `payload` into a `HGLOBAL` and hands it to the clipboard under `format`.
///
/// # Safety
/// Must be called with the clipboard open.
#[cfg(windows)]
unsafe fn set_bytes(format: u32, payload: &[u8]) -> Result<()> {
    unsafe {
        let hglobal = GlobalAlloc(GMEM_MOVEABLE, payload.len()).map_err(|e| {
            AppError::Clipboard(format!("could not allocate clipboard memory: {e}"))
        })?;
        let destination = GlobalLock(hglobal);
        if destination.is_null() {
            let _ = GlobalFree(Some(hglobal));
            return Err(AppError::Clipboard(
                "could not lock clipboard memory".into(),
            ));
        }
        std::ptr::copy_nonoverlapping(payload.as_ptr(), destination.cast::<u8>(), payload.len());
        let _ = GlobalUnlock(hglobal);

        if let Err(e) = SetClipboardData(format, Some(HANDLE(hglobal.0))) {
            let _ = GlobalFree(Some(hglobal));
            return Err(AppError::Clipboard(format!(
                "could not set clipboard format {format}: {e}"
            )));
        }
    }
    Ok(())
}

/// Publishes `payload` under a named registered format, e.g. "HTML Format".
#[cfg(windows)]
fn set_registered(name: &str, payload: &[u8]) -> Result<()> {
    let units = widened(name);
    let format = unsafe { RegisterClipboardFormatW(windows::core::PCWSTR(units.as_ptr())) };
    if format == 0 {
        return Err(AppError::Clipboard(format!(
            "the {name} clipboard format is unavailable"
        )));
    }
    let outcome = with_clipboard(|| unsafe { set_bytes(format, payload) });
    match outcome {
        Some(result) => result,
        None => Err(AppError::Clipboard(
            "another process is holding the clipboard open".into(),
        )),
    }
}

/// Wraps an HTML fragment in the `CF_HTML` header consumers expect.
///
/// The header records byte offsets, and its own length depends on how many digits
/// those offsets have, so it is rebuilt until the numbers stop changing.
fn build_cf_html(fragment: &str) -> Vec<u8> {
    const PROLOGUE: &str = "<html><body><!--StartFragment-->";
    const EPILOGUE: &str = "<!--EndFragment--></body></html>";
    let header = |start_html: usize,
                  end_html: usize,
                  start_fragment: usize,
                  end_fragment: usize| {
        format!(
            "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\n\
             StartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
        )
    };

    let mut start_html = 0;
    let mut end_html = 0;
    let mut start_fragment = 0;
    let mut end_fragment = 0;
    for _ in 0..3 {
        let length = header(start_html, end_html, start_fragment, end_fragment).len();
        start_html = length;
        start_fragment = length + PROLOGUE.len();
        end_fragment = start_fragment + fragment.len();
        end_html = end_fragment + EPILOGUE.len();
    }

    let mut out = header(start_html, end_html, start_fragment, end_fragment).into_bytes();
    out.extend_from_slice(PROLOGUE.as_bytes());
    out.extend_from_slice(fragment.as_bytes());
    out.extend_from_slice(EPILOGUE.as_bytes());
    out
}

#[cfg(windows)]
fn set_html(fragment: &str) -> Result<()> {
    set_registered("HTML Format", &build_cf_html(fragment))
}

#[cfg(windows)]
fn set_rtf(rtf: &str) -> Result<()> {
    set_registered("Rich Text Format", rtf.as_bytes())
}

/// Serialises paths into the `DROPFILES` + double-NUL-terminated UTF-16 list that
/// `CF_HDROP` consumers expect.
#[cfg(windows)]
fn build_hdrop(paths: &[String]) -> Vec<u8> {
    // DROPFILES: DWORD pFiles; POINT pt; BOOL fNC; BOOL fWide;
    const HEADER_SIZE: u32 = 20;
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&HEADER_SIZE.to_le_bytes());
    buffer.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    buffer.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    buffer.extend_from_slice(&0i32.to_le_bytes()); // fNC
    buffer.extend_from_slice(&1i32.to_le_bytes()); // fWide = TRUE
    for path in paths {
        for unit in path.encode_utf16() {
            buffer.extend_from_slice(&unit.to_le_bytes());
        }
        buffer.extend_from_slice(&0u16.to_le_bytes());
    }
    // A final extra NUL terminates the list itself.
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer
}

#[cfg(windows)]
fn set_files(paths: &[String]) -> Result<()> {
    let payload = build_hdrop(paths);
    unsafe {
        let hglobal = GlobalAlloc(GMEM_MOVEABLE, payload.len()).map_err(|e| {
            AppError::Clipboard(format!("could not allocate clipboard memory: {e}"))
        })?;
        let destination = GlobalLock(hglobal);
        if destination.is_null() {
            let _ = GlobalFree(Some(hglobal));
            return Err(AppError::Clipboard(
                "could not lock clipboard memory".into(),
            ));
        }
        std::ptr::copy_nonoverlapping(payload.as_ptr(), destination.cast::<u8>(), payload.len());
        let _ = GlobalUnlock(hglobal);

        if let Err(e) = SetClipboardData(CF_HDROP, Some(HANDLE(hglobal.0))) {
            let _ = GlobalFree(Some(hglobal));
            return Err(AppError::Clipboard(format!("could not set file list: {e}")));
        }
    }
    Ok(())
}

/// Serialises pixels into the `CF_DIB` payload a paste expects.
///
/// A `BITMAPINFOHEADER` with 32 bits per pixel and no compression, followed by the
/// rows bottom-up — which is the opposite order from every other image API — with
/// the channels swapped from the RGBA Kept stores to the BGRA the format wants.
/// The alpha channel is kept: a screenshot with rounded corners pastes with them
/// rather than on an opaque black rectangle.
#[cfg(windows)]
fn build_dib(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let stride = width as usize * 4;
    let expected = stride * height as usize;
    if width == 0 || height == 0 || rgba.len() != expected {
        return Err(AppError::Clipboard(format!(
            "a {width}×{height} image should hold {expected} bytes, not {}",
            rgba.len()
        )));
    }

    let mut payload = Vec::with_capacity(40 + expected);
    payload.extend_from_slice(&40u32.to_le_bytes()); // biSize
    payload.extend_from_slice(&(width as i32).to_le_bytes()); // biWidth
    payload.extend_from_slice(&(height as i32).to_le_bytes()); // biHeight, bottom-up
    payload.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    payload.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    payload.extend_from_slice(&0u32.to_le_bytes()); // biCompression, BI_RGB
    payload.extend_from_slice(&(expected as u32).to_le_bytes()); // biSizeImage
    payload.extend_from_slice(&[0u8; 16]); // pixels per metre, palette counts

    for row in (0..height as usize).rev() {
        let start = row * stride;
        let line = &rgba[start..start + stride];
        for pixel in line.chunks_exact(4) {
            payload.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    Ok(payload)
}

/// Puts a picture on the clipboard as a `CF_DIB`.
#[cfg(windows)]
fn set_image(width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let payload = build_dib(width, height, rgba)?;
    let outcome = with_clipboard(|| -> Result<()> {
        unsafe { EmptyClipboard()? };
        unsafe { set_bytes(CF_DIB, &payload) }
    });
    match outcome {
        Some(result) => result,
        None => Err(AppError::Clipboard(
            "another process is holding the clipboard open".into(),
        )),
    }
}

/// Process name (not full path) of the window that currently has focus.
#[cfg(windows)]
fn foreground_process_name() -> Option<String> {
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return None;
    }
    process_name_for(window.0 as isize)
}

/// Process name (not full path) of the window behind `handle`.
///
/// `None` when the owner cannot be queried at all, which is what a window belonging
/// to a higher-integrity process looks like from here.
#[cfg(windows)]
pub(crate) fn process_name_for(handle: isize) -> Option<String> {
    unsafe {
        let window = HWND(handle as *mut core::ffi::c_void);
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        if pid == 0 {
            return None;
        }

        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        result.ok()?;

        let full = String::from_utf16_lossy(&buffer[..length as usize]);
        Some(leaf_name(&full))
    }
}

#[cfg(not(windows))]
pub(crate) fn process_name_for(_handle: isize) -> Option<String> {
    None
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub fn sequence_number() -> u32 {
    macos_clipboard::sequence_number()
}

#[cfg(target_os = "macos")]
fn read_file_list() -> Option<Vec<String>> {
    macos_clipboard::file_list()
}

#[cfg(target_os = "macos")]
fn read_rich_text() -> (Option<String>, Option<String>) {
    macos_clipboard::rich_text()
}

#[cfg(target_os = "macos")]
fn clipboard_opt_out() -> bool {
    macos_clipboard::opt_out()
}

#[cfg(target_os = "macos")]
fn foreground_process_name() -> Option<String> {
    macos_clipboard::frontmost_app()
}

#[cfg(target_os = "macos")]
pub fn write_flavoured(
    content: &ClipContent,
    html: Option<&str>,
    rtf: Option<&str>,
) -> Result<String> {
    match content {
        ClipContent::Text(text) => {
            if !macos_clipboard::write_text(text, html, rtf) {
                return Err(AppError::Clipboard("the pasteboard refused the copy".into()));
            }
        }
        // arboard owns the image conversion, which is the one format where the
        // pasteboard's own types differ between a copy and a paste.
        ClipContent::Image {
            width,
            height,
            rgba,
        } => {
            let mut clip = arboard::Clipboard::new()
                .map_err(|e| AppError::Clipboard(format!("could not open the clipboard: {e}")))?;
            clip.set_image(arboard::ImageData {
                width: *width as usize,
                height: *height as usize,
                bytes: std::borrow::Cow::Borrowed(rgba.as_slice()),
            })
            .map_err(|e| AppError::Clipboard(e.to_string()))?;
        }
        ClipContent::Files(paths) => {
            if !macos_clipboard::write_files(paths) {
                return Err(AppError::Clipboard(
                    "the pasteboard refused the file list".into(),
                ));
            }
        }
    }
    Ok(hashing::of_content(content))
}

#[cfg(target_os = "macos")]
pub fn write_files(paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Err(AppError::Clipboard("there is no file to copy".into()));
    }
    if !macos_clipboard::write_files(paths) {
        return Err(AppError::Clipboard(
            "the pasteboard refused the file list".into(),
        ));
    }
    Ok(hashing::of_content(&ClipContent::Files(paths.to_vec())))
}

#[cfg(target_os = "macos")]
pub fn write_file_and_text(path: &Path, text: &str) -> Result<String> {
    let path = path.to_string_lossy().into_owned();
    if !macos_clipboard::write_file_and_text(&path, text) {
        return Err(AppError::Clipboard("the pasteboard refused the copy".into()));
    }
    Ok(hashing::of_content(&ClipContent::Files(vec![path])))
}

// ---------------------------------------------------------------------------
// Linux stubs
// ---------------------------------------------------------------------------

/// X11 has no clipboard sequence number, so the content itself is the signal:
/// whatever is on the clipboard is read and folded into a number. That costs a
/// clipboard read per poll where Windows reads a counter.
#[cfg(target_os = "linux")]
pub fn sequence_number() -> u32 {
    /// FNV-1a. Small, and enough to notice that the clipboard moved on.
    fn fold(seed: u32, bytes: &[u8]) -> u32 {
        bytes.iter().fold(seed, |acc, byte| {
            (acc ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
        })
    }

    if let Some(text) = read_text() {
        return fold(0x811c_9dc5, text.as_bytes());
    }
    // A file list offers no text, so it is asked for only when nothing else was
    // there: reading it costs an `xclip` call, which a text copy must not pay.
    if let Some(paths) = read_file_list() {
        let mut seed = 0x811c_9dc5;
        for path in paths {
            seed = fold(seed, path.as_bytes());
            seed = fold(seed, b"\n");
        }
        return seed;
    }
    match read_image() {
        Some(ClipContent::Image {
            width,
            height,
            rgba,
        }) => {
            let seed = fold(0x811c_9dc5, &width.to_le_bytes()) ^ height;
            fold(seed, &rgba)
        }
        _ => 0,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_text() -> Option<String> {
    let mut clip = arboard::Clipboard::new().ok()?;
    let text = clip.get_text().ok()?;
    let trimmed = text.trim().trim_matches('\0').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(target_os = "linux")]
fn read_file_list() -> Option<Vec<String>> {
    // arboard has no file-list support, so the target is read with `xclip`. A
    // failure means the clipboard is offering no such target, which is the usual
    // case and not worth a log line.
    let output = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "text/uri-list", "-o"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let paths = parse_uri_list(&String::from_utf8_lossy(&output.stdout));
    (!paths.is_empty()).then_some(paths)
}

#[cfg(target_os = "linux")]
fn read_rich_text() -> (Option<String>, Option<String>) {
    (None, None)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_image() -> Option<ClipContent> {
    // arboard serves X11 as well as Windows, so the image path is the same code.
    let mut clip = arboard::Clipboard::new().ok()?;
    let image = clip.get_image().ok()?;
    if image.width == 0 || image.height == 0 {
        return None;
    }
    let expected = image.width.checked_mul(image.height)?.checked_mul(4)?;
    if image.bytes.len() < expected {
        log::warn!(
            "clipboard image is {} bytes but {}x{} RGBA needs {expected}",
            image.bytes.len(),
            image.width,
            image.height
        );
        return None;
    }
    Some(ClipContent::Image {
        width: image.width as u32,
        height: image.height as u32,
        rgba: image.bytes[..expected].to_vec(),
    })
}

#[cfg(target_os = "linux")]
fn clipboard_opt_out() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn foreground_process_name() -> Option<String> {
    // The X11 focus owner's process is the closest thing to "the app you copied
    // from"; the name comes from /proc, which is what a Linux user would expect.
    let report = crate::paste::xdotool(&["getactivewindow", "getwindowpid"]).ok()?;
    let pid: u32 = report.split_whitespace().last()?.parse().ok()?;
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The X11 clipboard has one owner at a time, so the two tests that publish to
    /// it take turns instead of clobbering each other's selection.
    #[cfg(target_os = "linux")]
    static X11_CLIPBOARD: Mutex<()> = Mutex::new(());

    #[cfg(windows)]
    #[test]
    fn a_dib_is_bottom_up_and_channel_swapped() {        // 2×2: the top row is red then green, the bottom row blue then white.
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, // top
            0, 0, 255, 255, 255, 255, 255, 255, // bottom
        ];
        let dib = build_dib(2, 2, &rgba).unwrap();

        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 2);
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(dib[12..14].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(dib[14..16].try_into().unwrap()), 32);
        assert_eq!(u32::from_le_bytes(dib[16..20].try_into().unwrap()), 0);
        assert_eq!(dib.len(), 40 + 2 * 2 * 4);

        // The rows are stored the other way up, and each pixel as BGRA.
        assert_eq!(&dib[40..44], &[255, 0, 0, 255], "bottom-left is blue");
        assert_eq!(&dib[44..48], &[255, 255, 255, 255], "bottom-right is white");
        assert_eq!(&dib[48..52], &[0, 0, 255, 255], "top-left is red");
        assert_eq!(&dib[52..56], &[0, 255, 0, 255], "top-right is green");
    }

    #[cfg(windows)]
    #[test]
    fn a_dib_refuses_a_payload_that_does_not_fit_its_size() {
        assert!(build_dib(2, 2, &[0, 0, 0, 0]).is_err());
        assert!(build_dib(0, 4, &[]).is_err());
    }

    #[test]
    fn text_preview_is_collapsed_and_truncated() {
        let (preview, search_text) = describe(&ClipContent::Text("  hello \n\n world \t".into()));
        assert_eq!(preview, "hello world");
        assert_eq!(search_text, "hello world");

        let long = "x".repeat(PREVIEW_CHARS * 3);
        let (preview, _) = describe(&ClipContent::Text(long.clone()));
        assert_eq!(preview.chars().count(), PREVIEW_CHARS);
        assert_ne!(preview.len(), long.len());
    }

    #[test]
    fn text_preview_counts_characters_not_bytes() {
        // Multi-byte glyphs must not be truncated mid-character.
        let body = "é".repeat(PREVIEW_CHARS * 2);
        let (preview, _) = describe(&ClipContent::Text(body));
        assert_eq!(preview.chars().count(), PREVIEW_CHARS);
        assert!(preview.chars().all(|c| c == 'é'));
    }

    #[test]
    fn file_preview_summarises_lists() {
        let (one, search_text) = describe(&ClipContent::Files(vec!["C:\\docs\\report.pdf".into()]));
        assert_eq!(one, "report.pdf");
        assert_eq!(search_text, "c:\\docs\\report.pdf");

        let many = describe(&ClipContent::Files(vec![
            "C:\\a.txt".into(),
            "C:\\b.txt".into(),
            "C:\\c.txt".into(),
        ]));
        assert_eq!(many.0, "3 files · a.txt, b.txt, c.txt");
    }

    #[test]
    fn file_preview_truncates_long_lists() {
        let paths: Vec<String> = (0..10).map(|i| format!("C:\\file{i}.txt")).collect();
        let (preview, _) = describe(&ClipContent::Files(paths));
        assert!(preview.starts_with("10 files · "));
        assert!(
            !preview.contains("file9.txt"),
            "only the first few names are shown"
        );
    }

    #[test]
    fn image_preview_reports_dimensions() {
        let (preview, search_text) = describe(&ClipContent::Image {
            width: 1920,
            height: 1080,
            rgba: Vec::new(),
        });
        assert_eq!(preview, "Image 1920 × 1080");
        assert!(search_text.is_empty());
    }

    #[test]
    fn leaf_name_handles_both_separators() {
        assert_eq!(leaf_name("C:\\a\\b\\c.txt"), "c.txt");
        assert_eq!(leaf_name("C:/a/b/c.txt"), "c.txt");
        assert_eq!(leaf_name("plain.txt"), "plain.txt");
        assert_eq!(leaf_name(""), "");
    }

    #[test]
    fn is_self_matches_the_running_executable() {
        let own = std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(is_self(&own));
        assert!(is_self(&own.to_uppercase()));
        assert!(!is_self("definitely-not-us.exe"));
    }

    #[test]
    fn rich_text_offsets_survive_a_round_trip() {
        let fragment = "<b>Bold</b> and a caret →";
        let payload = build_cf_html(fragment);
        let text = String::from_utf8_lossy(&payload);
        let field = |key: &str| -> usize {
            let start = text.find(key).expect(key) + key.len();
            text[start..start + 10].parse().expect("a 10-digit offset")
        };
        let start = field("StartFragment:");
        let end = field("EndFragment:");
        assert_eq!(&text[start..end], fragment);
        assert_eq!(
            field("StartHTML:"),
            text.find("<html").expect("the document follows the header"),
            "StartHTML points at the document"
        );
        assert_eq!(field("EndHTML:"), payload.len());
        assert_eq!(cf_html_fragment(&payload).as_deref(), Some(fragment));
    }

    #[test]
    fn rich_text_falls_back_to_the_document_body() {
        let payload = b"Version:0.9\r\n<html><body><p>no offsets</p></body></html>";
        assert_eq!(
            cf_html_fragment(payload).as_deref(),
            Some("<body><p>no offsets</p></body></html>")
        );
    }

    #[test]
    fn pause_state_reports_a_deadline_and_resumes() {
        let watcher = Watcher::new();
        assert!(!watcher.pause_state().paused);

        watcher.pause_for(Duration::from_secs(30));
        let state = watcher.pause_state();
        assert!(state.paused);
        assert!(!state.indefinite);
        assert!(state.until.is_some_and(|until| until > 0));

        watcher.resume();
        assert!(!watcher.pause_state().paused);

        watcher.pause_until_restart();
        let state = watcher.pause_state();
        assert!(state.paused && state.indefinite);
        watcher.resume();
        assert!(!watcher.pause_state().paused);
    }

    #[cfg(windows)]
    #[test]
    fn hdrop_header_describes_a_wide_double_terminated_list() {
        let payload = build_hdrop(&["C:\\one.txt".into(), "C:\\two.txt".into()]);

        // DROPFILES layout: DWORD pFiles; POINT pt (2 x LONG); BOOL fNC; BOOL fWide.
        let field =
            |range: std::ops::Range<usize>| i32::from_le_bytes(payload[range].try_into().unwrap());
        assert_eq!(
            field(0..4),
            20,
            "pFiles points just past the 20-byte header"
        );
        assert_eq!(field(4..8), 0, "pt.x");
        assert_eq!(field(8..12), 0, "pt.y");
        assert_eq!(field(12..16), 0, "fNC");
        assert_eq!(field(16..20), 1, "fWide must be TRUE for UTF-16 paths");

        // Payload is UTF-16 with each path NUL-terminated plus a final NUL.
        let units: Vec<u16> = payload[20..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let text = String::from_utf16(&units).unwrap();
        assert_eq!(text, "C:\\one.txt\0C:\\two.txt\0\0");
    }

    #[cfg(windows)]
    #[test]
    fn hdrop_handles_an_empty_selection() {
        let payload = build_hdrop(&[]);
        assert_eq!(payload.len(), 22, "header plus the list terminator");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_path_becomes_a_percent_escaped_file_uri() {
        assert_eq!(file_uri("/tmp/plain.mp4"), "file:///tmp/plain.mp4");
        assert_eq!(
            file_uri("/home/me/My Recordings/a b.mp4"),
            "file:///home/me/My%20Recordings/a%20b.mp4"
        );
        // Reserved characters a file manager would otherwise read as syntax.
        assert_eq!(file_uri("/tmp/a#b?c"), "file:///tmp/a%23b%3Fc");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_uri_list_reads_back_as_the_paths_it_names() {
        let listing = "# copied from Kept\r\nfile:///tmp/one.png\r\nfile:///tmp/two%20words.pdf\r\n";
        assert_eq!(
            parse_uri_list(listing),
            vec!["/tmp/one.png".to_string(), "/tmp/two words.pdf".to_string()]
        );
        assert_eq!(parse_uri_list("file:///tmp/x\r\n\r\n"), vec!["/tmp/x".to_string()]);
        assert!(parse_uri_list("").is_empty());
        // A remote URI is not a local path, so it is left out rather than mangled.
        assert!(parse_uri_list("https://example.com/x.png").is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "writes to the live X clipboard; run with DISPLAY set"]
    fn a_file_list_round_trips_through_the_x11_clipboard() {
        let _clipboard = X11_CLIPBOARD.lock();
        // `xclip` reads its data from a file descriptor and then stays resident to
        // serve the selection, so it is not waited on: only the data it published
        // is what this test is about.
        let listing = std::env::temp_dir().join("kept-uri-list.txt");
        let contents = "file:///tmp/kept-uri-list-one.txt\r\nfile:///tmp/kept%20two.txt\r\n";
        if std::fs::write(&listing, contents).is_err() {
            return;
        }
        let Ok(handle) = std::fs::File::open(&listing) else {
            return;
        };
        let mut writer = match std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "text/uri-list", "-i"])
            .stdin(std::process::Stdio::from(handle))
            // The copy `xclip` forks to serve the selection would otherwise hold
            // the test's own pipes open.
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            // No xclip on this machine, so there is nothing to test.
            Err(_) => return,
        };
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            if read_file_list().is_some() {
                break;
            }
        }
        let _ = writer.kill();
        let _ = std::fs::remove_file(&listing);

        assert_eq!(
            read_file_list(),
            Some(vec![
                "/tmp/kept-uri-list-one.txt".to_string(),
                "/tmp/kept two.txt".to_string(),
            ]),
            "the file list a copy publishes has to be the one the watcher reads"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "writes to the live X clipboard; run with DISPLAY set"]
    fn the_copied_images_html_round_trips_through_the_x11_clipboard() {
        let _clipboard = X11_CLIPBOARD.lock();
        // Exactly what Chromium publishes for "Copy image": the frame it shows,
        // plus the tag naming the asset the frame came from.
        let page = std::env::temp_dir().join("kept-motion-html.html");
        let contents = r#"<img src="https://media.example.com/a.gif"/>"#;
        if std::fs::write(&page, contents).is_err() {
            return;
        }
        let Ok(handle) = std::fs::File::open(&page) else {
            return;
        };
        let mut writer = match std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "text/html", "-i"])
            .stdin(std::process::Stdio::from(handle))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            // No xclip on this machine, so there is nothing to test.
            Err(_) => return,
        };
        let mut html = None;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            if let Some(found) = motion_html() {
                html = Some(found);
                break;
            }
        }
        let _ = writer.kill();
        let _ = std::fs::remove_file(&page);

        assert_eq!(
            html.as_deref().and_then(animated::find),
            Some(animated::MotionSource::Remote {
                kind: animated::MotionKind::Gif,
                url: "https://media.example.com/a.gif".to_string(),
            }),
            "the address of a copied GIF has to be recoverable from the clipboard"
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "writes to the live system clipboard; run explicitly"]
    fn round_trip_text_through_the_real_clipboard() {
        // Exercises allocation, the Win32 write path, and the read path together.
        let Some(_) = with_clipboard(|| ()) else {
            // Another process is monopolising the clipboard; skip rather than fail.
            return;
        };

        const SAMPLE: &str = "line one\r\nline two\ttabbed ✓ 日本語";
        if write(&ClipContent::Text(SAMPLE.into())).is_err() {
            return;
        }
        assert_eq!(
            read_text().as_deref(),
            Some(SAMPLE),
            "CRLF and unicode must survive a clipboard round trip"
        );
    }
}

//! The `::` auto-complete popup's trigger.
//!
//! Everything here happens before the popup exists: a low-level keyboard hook
//! watches the characters the keyboard produces in whichever application has the
//! focus, notices the configured token, takes that token back out of the document,
//! remembers the window the paste should land in, and only then opens the popup.
//!
//! Two rules shape the code. First, the hook is installed only while the feature
//! is switched on, and it keeps nothing: it compares the last few characters
//! against the token and forgets them. Nothing is logged, buffered or sent
//! anywhere. Second, a low-level hook runs on the thread that installed it and
//! blocks every keystroke on the machine until it returns, so it may not open
//! windows or sleep — the work happens on the worker thread it signals.

use std::sync::atomic::AtomicBool;
#[cfg(any(windows, target_os = "linux"))]
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
#[cfg(any(windows, target_os = "linux"))]
use std::sync::mpsc;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::AppHandle;

#[cfg(any(windows, target_os = "linux"))]
use crate::paste;
use crate::settings::{Settings, MAX_TOKEN_CHARS};
#[cfg(any(windows, target_os = "linux"))]
use crate::window;

/// How quickly the token's characters have to arrive to count as one trigger. Long
/// enough for a two-fingered `::`, short enough that the same two characters typed
/// minutes apart are not one.
const TOKEN_WINDOW: Duration = Duration::from_millis(1_500);

/// Time the application is given to put the typed characters into its document
/// before the backspaces that remove them are sent.
const TOKEN_SETTLE: Duration = Duration::from_millis(120);

/// Time the backspaces are given to land before the popup takes the focus away.
const BACKSPACE_SETTLE: Duration = Duration::from_millis(40);

/// What the hook may do, mirrored from the settings whenever they change.
#[derive(Debug, Clone)]
struct Config {
    token: Vec<char>,
    denied: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            token: "::".chars().collect(),
            denied: Vec::new(),
        }
    }
}

/// Characters typed since the last non-matching key, with the moment the newest
/// one arrived.
#[derive(Debug, Default)]
struct Strokes {
    chars: Vec<char>,
    at: Option<Instant>,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static CONFIG: LazyLock<Mutex<Config>> = LazyLock::new(|| Mutex::new(Config::default()));
static STROKES: Mutex<Strokes> = Mutex::new(Strokes {
    chars: Vec::new(),
    at: None,
});
/// Set once: the worker that reacts to a trigger. The sender outlives every
/// enable/disable cycle, so switching the feature off and on again reuses it.
static TRIGGER: OnceLock<Sender<()>> = OnceLock::new();

/// Turns the trigger on or off and adopts a changed token or deny-list.
///
/// Called at startup and after every settings write, so the hook never runs on a
/// configuration the user has already replaced.
#[cfg(windows)]
pub fn apply(app: &AppHandle, settings: &Settings) {
    {
        let mut config = lock(&CONFIG);
        *config = Config {
            token: settings.quick_paste_token.chars().collect(),
            denied: settings.quick_paste_denied_apps.clone(),
        };
    }
    // The foreground cache answers with the deny-list it was computed against, so a
    // changed list has to invalidate it.
    invalidate_foreground();

    if settings.quick_paste_enabled {
        ensure_started(app);
        ENABLED.store(true, Ordering::SeqCst);
    } else {
        // Off means no hook is installed at all, not a hook that ignores keys.
        ENABLED.store(false, Ordering::SeqCst);
        stop();
    }
}

#[cfg(target_os = "linux")]
pub fn apply(app: &AppHandle, settings: &Settings) {
    {
        let mut config = lock(&CONFIG);
        *config = Config {
            token: settings.quick_paste_token.chars().collect(),
            denied: settings.quick_paste_denied_apps.clone(),
        };
    }
    if settings.quick_paste_enabled {
        ensure_started(app);
        ENABLED.store(true, Ordering::SeqCst);
    } else {
        // Off means the recording context is gone, not a context that ignores keys.
        ENABLED.store(false, Ordering::SeqCst);
        stop();
    }
}

/// macOS has no recorder for another application's keystrokes yet, so the token
/// and deny-list are kept up to date and nothing is watched: the `::` popup stays
/// unavailable there rather than pretending to be armed.
#[cfg(not(any(windows, target_os = "linux")))]
pub fn apply(_app: &AppHandle, settings: &Settings) {
    let mut config = lock(&CONFIG);
    *config = Config {
        token: settings.quick_paste_token.chars().collect(),
        denied: settings.quick_paste_denied_apps.clone(),
    };
}

/// Stops the hook; used on the way out of the process.
pub fn stop() {
    #[cfg(windows)]
    {
        let hook = lock(&HOOK).take();
        if let Some(hook) = hook {
            // The hook thread owns its message loop, so the only way to end it is
            // to ask its queue to quit.
            unsafe {
                let _ = win32::PostThreadMessageW(hook.thread, win32::WM_QUIT, Default::default(), Default::default());
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let hook = lock(&HOOK).take();
        if let Some(hook) = hook {
            // The context is a server-side object, so a second connection can end
            // the loop the observing thread is sitting in.
            match x11rb::connect(None) {
                Ok((conn, _)) => {
                    use x11rb::protocol::record::ConnectionExt as _;
                    use x11rb::wrapper::ConnectionExt as _;
                    let _ = conn.record_disable_context(hook.context);
                    let _ = conn.sync();
                }
                Err(e) => log::warn!("could not ask the X server to stop recording keys: {e}"),
            }
        }
    }
}

/// The hook's own state: the thread whose message loop has to be ended.
#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
struct Hook {
    thread: u32,
}

/// The recording context the observer is iterating.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct Hook {
    context: u32,
}

#[cfg(any(windows, target_os = "linux"))]
static HOOK: Mutex<Option<Hook>> = Mutex::new(None);

/// Starts the hook thread unless it is already running.
#[cfg(windows)]
fn ensure_started(app: &AppHandle) {
    trigger_sender(app);
    if lock(&HOOK).is_some() {
        return;
    }
    let (sender, receiver) = mpsc::channel::<u32>();
    let spawned = std::thread::Builder::new()
        .name("quick-hook".into())
        .spawn(move || run_hook(sender));
    if let Err(e) = spawned {
        log::warn!("could not start the quick-popup keyboard hook: {e}");
        return;
    }
    // A zero means the hook could not be installed; the thread reports back so the
    // failure is logged once here rather than on every keystroke.
    match receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(thread) if thread != 0 => *lock(&HOOK) = Some(Hook { thread }),
        Ok(_) => log::warn!("the quick-popup keyboard hook was refused"),
        Err(e) => log::warn!("the quick-popup keyboard hook did not start: {e}"),
    }
}

#[cfg(windows)]
fn run_hook(sender: Sender<u32>) {
    unsafe {
        let hook = match win32::SetWindowsHookExW(win32::WH_KEYBOARD_LL, Some(hook_proc), None, 0) {
            Ok(hook) => hook,
            Err(e) => {
                log::warn!("could not install the quick-popup keyboard hook: {e}");
                let _ = sender.send(0);
                return;
            }
        };
        let _ = sender.send(win32::GetCurrentThreadId());

        let mut message = win32::MSG::default();
        // A low-level hook is delivered to the thread that installed it, so this
        // loop is what keeps it alive; there is no window behind it.
        while win32::GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = win32::TranslateMessage(&message);
            let _ = win32::DispatchMessageW(&message);
        }
        let _ = win32::UnhookWindowsHookEx(hook);
    }
}

/// Answers with the channel the hook signals, starting the worker the first time.
#[cfg(windows)]
fn trigger_sender(app: &AppHandle) -> &'static Sender<()> {
    TRIGGER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<()>();
        let app = app.clone();
        let spawned = std::thread::Builder::new()
            .name("quick-popup".into())
            .spawn(move || {
                // One trigger at a time: a fast typist pressing `::` twice must not
                // queue two popups.
                while receiver.recv().is_ok() {
                    trigger(&app);
                }
            });
        if let Err(e) = spawned {
            log::warn!("could not start the quick-popup worker: {e}");
        }
        sender
    })
}

/// Everything that has to happen between noticing the token and showing the popup.
#[cfg(windows)]
fn trigger(app: &AppHandle) {
    if window::quick_is_open(app) {
        return;
    }
    // Remember the window to paste back into while it still has the focus: the
    // popup is about to take it.
    paste::capture_foreground(None);
    // Where to hang the popup, measured before anything moves.
    let origin = caret_point();
    let length = lock(&CONFIG).token.len();

    // The typed characters have to be in the document before the backspaces remove
    // them, or the backspaces eat whatever came before them instead.
    std::thread::sleep(TOKEN_SETTLE);
    if let Err(e) = paste::send_backspaces(length) {
        log::warn!("could not remove the `::` from the document: {e}");
    }
    std::thread::sleep(BACKSPACE_SETTLE);
    window::show_quick(app, origin);
}

#[cfg(target_os = "linux")]
fn ensure_started(app: &AppHandle) {
    trigger_sender(app);
    if lock(&HOOK).is_some() {
        return;
    }
    let (sender, receiver) = mpsc::channel::<u32>();
    let spawned = std::thread::Builder::new()
        .name("quick-hook".into())
        .spawn(move || observe(sender));
    if let Err(e) = spawned {
        log::warn!("could not start the quick-popup key observer: {e}");
        return;
    }
    // Zero means the observer could not be installed; the thread reports back so
    // the failure is logged once here rather than on every keystroke.
    match receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(context) if context != 0 => *lock(&HOOK) = Some(Hook { context }),
        Ok(_) => log::warn!("the X server refused a recording context for the quick popup"),
        Err(e) => log::warn!("the quick-popup key observer did not start: {e}"),
    }
}

/// Watches every key the X server delivers, until the recording context is
/// disabled.
///
/// RECORD needs two connections: one owns the context, and the other reads the
/// event stream. Nothing is grabbed, so typing is never held up and a keystroke
/// cannot be lost while the popup decides what to do with it.
#[cfg(target_os = "linux")]
fn observe(sender: Sender<u32>) {
    use x11rb::connection::{Connection, RequestConnection};
    use x11rb::protocol::record::{self, ConnectionExt as _};
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::x11_utils::TryParse;

    let fail = |message: &str| {
        log::warn!("{message}");
        let _ = sender.send(0);
    };
    let (control, _) = match x11rb::connect(None) {
        Ok(pair) => pair,
        Err(e) => return fail(&format!("could not open the X connection for the quick popup: {e}")),
    };
    let (stream, _) = match x11rb::connect(None) {
        Ok(pair) => pair,
        Err(e) => return fail(&format!("could not open the second X connection: {e}")),
    };
    match control.extension_information(record::X11_EXTENSION_NAME) {
        Ok(Some(_)) => {}
        Ok(None) => return fail("the X server has no RECORD extension; :: quick paste needs it"),
        Err(e) => return fail(&format!("could not query the RECORD extension: {e}")),
    }

    // The keyboard map answers what each keycap produces, which is what a keycode
    // alone cannot say.
    let map = match control
        .get_keyboard_mapping(8, 248)
        .map_err(|e| e.to_string())
        .and_then(|cookie| cookie.reply().map_err(|e| e.to_string()))
    {
        Ok(map) => map,
        Err(e) => return fail(&format!("could not read the keyboard map: {e}")),
    };
    let keysyms = Keymap::new(8, map.keysyms_per_keycode, map.keysyms);

    let context = match control.generate_id() {
        Ok(id) => id,
        Err(e) => return fail(&format!("could not allocate a recording context: {e}")),
    };
    let empty = record::Range8 { first: 0, last: 0 };
    let empty_ext = record::ExtRange {
        major: empty,
        minor: record::Range16 { first: 0, last: 0 },
    };
    let range = record::Range {
        core_requests: empty,
        core_replies: empty,
        ext_requests: empty_ext,
        ext_replies: empty_ext,
        delivered_events: empty,
        device_events: record::Range8 {
            first: x11rb::protocol::xproto::KEY_PRESS_EVENT,
            last: x11rb::protocol::xproto::KEY_RELEASE_EVENT,
        },
        errors: empty,
        client_started: false,
        client_died: false,
    };
    let created = control
        .record_create_context(context, 0, &[record::CS::ALL_CLIENTS.into()], &[range])
        .map_err(|e| e.to_string())
        .and_then(|cookie| cookie.check().map_err(|e| e.to_string()));
    if let Err(e) = created {
        return fail(&format!("could not record keys: {e}"));
    }
    // Handed back before the stream is read, because reading it never returns.
    let _ = sender.send(context);

    let replies = match stream.record_enable_context(context) {
        Ok(replies) => replies,
        Err(e) => {
            log::warn!("could not start reading recorded keys: {e}");
            return;
        }
    };
    for reply in replies {
        let Ok(reply) = reply else { break };
        // Category zero is the event stream itself; anything else is the server
        // talking about the recording rather than through it.
        if reply.category != 0 {
            continue;
        }
        let mut remaining = &reply.data[..];
        while !remaining.is_empty() {
            match remaining[0] {
                x11rb::protocol::xproto::KEY_PRESS_EVENT => {
                    let Ok((event, rest)) = x11rb::protocol::xproto::KeyPressEvent::try_parse(remaining)
                    else {
                        break;
                    };
                    if ENABLED.load(Ordering::Relaxed) {
                        if let Some(typed) = keysyms.character(event.detail, event.state.into()) {
                            observe_character(typed);
                        }
                    }
                    remaining = rest;
                }
                x11rb::protocol::xproto::KEY_RELEASE_EVENT => {
                    let Ok((_event, rest)) =
                        x11rb::protocol::xproto::KeyReleaseEvent::try_parse(remaining)
                    else {
                        break;
                    };
                    remaining = rest;
                }
                // The ranges asked for key events only, so anything else means the
                // stream is not laid out the way this loop assumes.
                other => {
                    log::debug!("unexpected recorded event {other}; ending the observer");
                    return;
                }
            }
        }
    }
    log::info!("the quick-popup key observer stopped");
}

/// The keyboard map, so a keycode plus the modifiers held can name a character.
#[cfg(target_os = "linux")]
struct Keymap {
    first_keycode: u8,
    per_keycode: usize,
    keysyms: Vec<u32>,
}

#[cfg(target_os = "linux")]
impl Keymap {
    fn new(first_keycode: u8, per_keycode: u8, keysyms: Vec<u32>) -> Self {
        Self {
            first_keycode,
            per_keycode: per_keycode.max(1) as usize,
            keysyms,
        }
    }

    /// `state` is the modifier mask the event carried; shift picks the second
    /// column, which is what makes Shift+`;` a colon on a US layout. The other
    /// modifiers are left to the layout: this only has to notice a punctuation
    /// token, not reimplement a keyboard.
    fn character(&self, keycode: u8, state: u16) -> Option<char> {
        let shifted = state & 0x1 != 0;
        let index = (keycode as usize).checked_sub(self.first_keycode as usize)?;
        let column = index * self.per_keycode + usize::from(shifted);
        let keysym = *self.keysyms.get(column)?;
        char_of(keysym)
    }
}

/// The character a keysym stands for, in the two forms a token can be made of.
#[cfg(target_os = "linux")]
fn char_of(keysym: u32) -> Option<char> {
    // Latin-1 is its own keysym range.
    if (0x20..=0x7e).contains(&keysym) {
        return char::from_u32(keysym);
    }
    // Everything else arrives as the Unicode keysym form: 0x01000000 | codepoint.
    if keysym & 0xff00_0000 == 0x0100_0000 {
        return char::from_u32(keysym & 0x00ff_ffff);
    }
    None
}

/// Checks one typed character against the token, exactly as the Windows hook does.
#[cfg(target_os = "linux")]
fn observe_character(typed: char) {
    if !target_allows_trigger() {
        return;
    }
    let fired = {
        let config = lock(&CONFIG);
        let mut strokes = lock(&STROKES);
        record(&mut strokes, &config.token, typed, Instant::now())
    };
    if fired {
        lock(&STROKES).chars.clear();
        if let Some(sender) = TRIGGER.get() {
            // Never blocks: the worker does the work, this thread must stay free.
            let _ = sender.send(());
        }
    }
}

/// Whether the token typed into the focused window should open the popup.
#[cfg(target_os = "linux")]
fn target_allows_trigger() -> bool {
    // Our own windows — the popup, the history overlay, the settings page — must
    // never open a second popup.
    let Some(pid) = foreground_pid() else {
        return false;
    };
    if pid == std::process::id() {
        return false;
    }
    let Ok(config) = CONFIG.try_lock() else {
        return false;
    };
    match std::fs::read_to_string(format!("/proc/{pid}/comm")) {
        Ok(name) => {
            let name = name.trim();
            !config
                .denied
                .iter()
                .any(|denied| denied.eq_ignore_ascii_case(name))
        }
        // A process this one cannot name is not one a paste could be tested
        // against, so the token is left where it was typed.
        Err(_) => false,
    }
}

/// The process that owns the X input focus.
#[cfg(target_os = "linux")]
fn foreground_pid() -> Option<u32> {
    let report = paste::xdotool(&["getactivewindow", "getwindowpid"]).ok()?;
    report.split_whitespace().last()?.parse().ok()
}

/// Everything that has to happen between noticing the token and showing the popup,
/// the X11 way: there is no caret to ask for, so the popup hangs off the pointer.
#[cfg(target_os = "linux")]
fn trigger(app: &AppHandle) {
    if window::quick_is_open(app) {
        return;
    }
    // Remember the window to paste back into while it still has the focus: the
    // popup is about to take it.
    paste::capture_foreground(None);
    let origin = pointer_point();
    let length = lock(&CONFIG).token.len();

    // The typed characters have to be in the document before the backspaces remove
    // them, or the backspaces eat whatever came before them instead.
    std::thread::sleep(TOKEN_SETTLE);
    if let Err(e) = paste::send_backspaces(length) {
        log::warn!("could not remove the `::` from the document: {e}");
    }
    std::thread::sleep(BACKSPACE_SETTLE);
    window::show_quick(app, origin);
}

#[cfg(target_os = "linux")]
fn pointer_point() -> Option<(i32, i32)> {
    let report = paste::xdotool(&["getmouselocation", "--shell"]).ok()?;
    let value = |key: &str| {
        report.lines().find_map(|line| {
            let value = line.trim().strip_prefix(key)?.strip_prefix('=')?;
            value.trim().parse().ok()
        })
    };
    Some((value("X")?, value("Y")?))
}

/// Answers with the channel the hook signals, starting the worker the first time.
#[cfg(target_os = "linux")]
fn trigger_sender(app: &AppHandle) -> &'static Sender<()> {
    TRIGGER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<()>();
        let app = app.clone();
        let spawned = std::thread::Builder::new()
            .name("quick-popup".into())
            .spawn(move || {
                // One trigger at a time: a fast typist pressing `::` twice must not
                // queue two popups.
                while receiver.recv().is_ok() {
                    trigger(&app);
                }
            });
        if let Err(e) = spawned {
            log::warn!("could not start the quick-popup worker: {e}");
        }
        sender
    })
}

/// True when the character completes the token.
///
/// Split out from the hook so the matching rule can be tested without a keyboard.
/// A gap longer than [`TOKEN_WINDOW`] starts a new run, which is what keeps two
/// colons typed minutes apart from opening the popup.
fn record(strokes: &mut Strokes, token: &[char], typed: char, now: Instant) -> bool {
    if token.is_empty() {
        return false;
    }
    let stale = strokes
        .at
        .is_none_or(|at| now.duration_since(at) > TOKEN_WINDOW);
    if stale {
        strokes.chars.clear();
    }
    strokes.chars.push(typed);
    strokes.at = Some(now);

    let keep = MAX_TOKEN_CHARS.max(token.len());
    if strokes.chars.len() > keep {
        let excess = strokes.chars.len() - keep;
        strokes.chars.drain(0..excess);
    }
    let tail = &strokes.chars[strokes.chars.len().saturating_sub(token.len())..];
    tail == token
}

#[cfg(windows)]
unsafe extern "system" fn hook_proc(code: i32, wparam: win32::WPARAM, lparam: win32::LPARAM) -> win32::LRESULT {
    // A negative code means the hook is being asked to pass the event on untouched.
    if code >= 0 && ENABLED.load(Ordering::Relaxed) {
        let message = wparam.0 as u32;
        if message == win32::WM_KEYDOWN || message == win32::WM_SYSKEYDOWN {
            let info = unsafe { &*(lparam.0 as *const win32::KBDLLHOOKSTRUCT) };
            // Injected events are other software typing — including our own
            // backspaces, which would otherwise look like keystrokes.
            if info.flags.0 & win32::LLKHF_INJECTED.0 == 0 {
                observe(info.vkCode, info.scanCode);
            }
        }
    }
    unsafe { win32::CallNextHookEx(None, code, wparam, lparam) }
}

/// Turns one key press into a character and checks it against the token.
#[cfg(windows)]
fn observe(key: u32, scan: u32) {
    if !target_allows_trigger() {
        return;
    }
    let Some(typed) = character_for(key, scan) else {
        return;
    };
    let fired = {
        let config = lock(&CONFIG);
        let mut strokes = lock(&STROKES);
        record(&mut strokes, &config.token, typed, Instant::now())
    };
    if fired {
        lock(&STROKES).chars.clear();
        if let Some(sender) = TRIGGER.get() {
            // Never blocks: the worker does the work, this thread must stay free.
            let _ = sender.send(());
        }
    }
}

/// The character a key press produces in the foreground application's layout.
///
/// The hook thread owns no input focus, so the queue state it can read knows
/// nothing about the modifiers the user is holding; those are taken from the async
/// key state instead, which is what makes Shift+`;` a colon.
#[cfg(windows)]
fn character_for(key: u32, scan: u32) -> Option<char> {
    use win32::{
        GetAsyncKeyState, GetKeyboardLayout, GetKeyboardState, MAPVK_VK_TO_VSC, MapVirtualKeyW,
        ToUnicodeEx, VK_SPACE,
    };

    unsafe {
        let foreground = win32::GetForegroundWindow();
        if foreground.is_invalid() {
            return None;
        }
        let thread = win32::GetWindowThreadProcessId(foreground, None);
        let layout = GetKeyboardLayout(thread);

        let mut state = [0u8; 256];
        let _ = GetKeyboardState(&mut state);
        for virtual_key in [
            win32::VK_SHIFT,
            win32::VK_LSHIFT,
            win32::VK_RSHIFT,
            win32::VK_CONTROL,
            win32::VK_LCONTROL,
            win32::VK_RCONTROL,
            win32::VK_MENU,
            win32::VK_LMENU,
            win32::VK_RMENU,
            win32::VK_CAPITAL,
        ] {
            let index = virtual_key.0 as usize;
            state[index] = if GetAsyncKeyState(virtual_key.0 as i32) < 0 {
                0x80
            } else {
                0
            };
        }

        let mut buffer = [0u16; 8];
        let count = ToUnicodeEx(key, scan, &state, &mut buffer, 0, Some(layout));
        // Zero is a key with no character; negative is a dead key. A dead key left
        // pending in the layout would attach itself to the application's own
        // conversion of the next character, so converting a space clears it.
        if count < 0 {
            let mut flush = [0u16; 8];
            let space = MapVirtualKeyW(VK_SPACE.0 as u32, MAPVK_VK_TO_VSC) as u16;
            let _ = ToUnicodeEx(VK_SPACE.0 as u32, u32::from(space), &state, &mut flush, 0, Some(layout));
            return None;
        }
        if count == 0 {
            return None;
        }
        char::from_u32(u32::from(buffer[0]))
    }
}

/// Whether the token typed into the window that has the focus should open the popup.
#[cfg(windows)]
fn target_allows_trigger() -> bool {
    unsafe {
        let foreground = win32::GetForegroundWindow();
        if foreground.is_invalid() {
            return false;
        }
        let handle = foreground.0 as isize;
        // Recomputed only when the focus moves: naming a process for every
        // keystroke would put an OpenProcess on the critical path of the whole
        // machine's typing.
        if let Ok(cache) = FOREGROUND.try_lock()
            && cache.0 == handle
        {
            return cache.1;
        }
        let allowed = decide(foreground);
        if let Ok(mut cache) = FOREGROUND.try_lock() {
            *cache = (handle, allowed);
        }
        allowed
    }
}

#[cfg(windows)]
fn decide(foreground: win32::HWND) -> bool {
    let mut pid = 0u32;
    unsafe { win32::GetWindowThreadProcessId(foreground, Some(&mut pid)) };
    // Our own windows: the popup, the history overlay, the settings page. Typing in
    // those must never open a second popup.
    if pid == 0 || pid == std::process::id() {
        return false;
    }
    // A window this process cannot even name is one with higher integrity, and a
    // paste could never land there anyway — better to leave the token in the
    // document than to swallow it and paste nothing.
    let Some(name) = crate::clipboard::process_name_for(foreground.0 as isize) else {
        return false;
    };
    let Ok(config) = CONFIG.try_lock() else {
        return false;
    };
    !config
        .denied
        .iter()
        .any(|denied| denied.eq_ignore_ascii_case(&name))
}

#[cfg(windows)]
static FOREGROUND: Mutex<(isize, bool)> = Mutex::new((0, false));

#[cfg(windows)]
fn invalidate_foreground() {
    if let Ok(mut cache) = FOREGROUND.try_lock() {
        *cache = (0, false);
    }
}

/// Screen point of the text caret in the focused application, when it can be found.
///
/// The caret rectangle is only askable of the thread that owns it, and that thread
/// is another process. Attaching to its input briefly is the documented way in:
/// while attached, `GetCaretPos` answers for its focus, and the point it gives is
/// in that window's client coordinates.
#[cfg(windows)]
fn caret_point() -> Option<(i32, i32)> {
    use win32::{
        AttachThreadInput, ClientToScreen, GetCaretPos, GetCurrentThreadId, GetGUIThreadInfo,
        GUITHREADINFO, POINT,
    };

    unsafe {
        let foreground = win32::GetForegroundWindow();
        if foreground.is_invalid() {
            return None;
        }
        let thread = win32::GetWindowThreadProcessId(foreground, None);
        if thread == 0 {
            return None;
        }
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(thread, &mut info).is_err() {
            return None;
        }
        let focus = if info.hwndFocus.is_invalid() {
            foreground
        } else {
            info.hwndFocus
        };

        let ours = GetCurrentThreadId();
        let attached = AttachThreadInput(ours, thread, true).as_bool();
        let mut point = POINT::default();
        let found = GetCaretPos(&mut point).is_ok();
        if attached {
            let _ = AttachThreadInput(ours, thread, false);
        }
        if !found || !ClientToScreen(focus, &mut point).as_bool() {
            return None;
        }
        Some((point.x, point.y))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The Win32 surface this module needs, named once.
#[cfg(windows)]
mod win32 {
    pub use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    pub use windows::Win32::Graphics::Gdi::ClientToScreen;
    pub use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    pub use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, GetKeyboardLayout, GetKeyboardState, MAPVK_VK_TO_VSC, MapVirtualKeyW,
        ToUnicodeEx, VK_CAPITAL, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RCONTROL,
        VK_RMENU, VK_RSHIFT, VK_SHIFT, VK_SPACE,
    };
    pub use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetCaretPos, GetForegroundWindow, GetGUIThreadInfo,
        GetMessageW, GetWindowThreadProcessId, PostThreadMessageW, SetWindowsHookExW,
        TranslateMessage, UnhookWindowsHookEx, GUITHREADINFO, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
        WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn two_colons_in_a_row_complete_the_token() {
        let mut strokes = Strokes::default();
        let now = Instant::now();
        assert!(!record(&mut strokes, &token("::"), ':', now));
        assert!(record(
            &mut strokes,
            &token("::"),
            ':',
            now + Duration::from_millis(120)
        ));
    }

    #[test]
    fn an_interrupted_token_does_not_complete() {
        let mut strokes = Strokes::default();
        let now = Instant::now();
        record(&mut strokes, &token("::"), ':', now);
        record(&mut strokes, &token("::"), 'a', now + Duration::from_millis(80));
        assert!(!record(
            &mut strokes,
            &token("::"),
            ':',
            now + Duration::from_millis(160)
        ));
    }

    #[test]
    fn a_run_longer_than_the_token_still_completes() {
        // `1:2:3::` ends with the token, so it still fires on the last character.
        let mut strokes = Strokes::default();
        let mut now = Instant::now();
        let token = token("::");
        for typed in ['1', ':', '2', ':', '3', ':'] {
            assert!(!record(&mut strokes, &token, typed, now));
            now += Duration::from_millis(60);
        }
        assert!(record(&mut strokes, &token, ':', now));
    }

    #[test]
    fn characters_typed_minutes_apart_are_not_a_token() {
        let mut strokes = Strokes::default();
        let now = Instant::now();
        record(&mut strokes, &token("::"), ':', now);
        assert!(!record(&mut strokes, &token("::"), ':', now + TOKEN_WINDOW * 2));
        // The late colon starts a new run, which the next one completes.
        assert!(record(
            &mut strokes,
            &token("::"),
            ':',
            now + TOKEN_WINDOW * 2 + Duration::from_millis(60)
        ));
    }

    #[test]
    fn an_empty_token_never_fires() {
        let mut strokes = Strokes::default();
        assert!(!record(&mut strokes, &[], ':', Instant::now()));
    }

    /// X11 reports a keycode and the modifiers held, so the character has to be
    /// looked up in the keyboard map: colon is the shifted column of `;`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_keycode_and_its_modifiers_name_the_character_typed() {
        // Two keycodes, seven columns each: `a` and `;`/`:`.
        let mut keysyms = vec![0u32; 14];
        keysyms[0] = 0x61; // a
        keysyms[7] = 0x3b; // ;
        keysyms[8] = 0x3a; // :
        let map = Keymap::new(8, 7, keysyms);

        assert_eq!(map.character(8, 0), Some('a'));
        assert_eq!(map.character(9, 0), Some(';'));
        assert_eq!(map.character(9, 1), Some(':'), "shift picks the second column");
        assert_eq!(map.character(7, 0), None, "below the first keycode");
        assert_eq!(map.character(200, 0), None, "past the end of the map");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_keysym_becomes_the_character_it_stands_for() {
        assert_eq!(char_of(0x3a), Some(':'));
        assert_eq!(char_of(0x7e), Some('~'));
        // Unicode keysyms carry the codepoint in the low three bytes.
        assert_eq!(char_of(0x0100_00e9), Some('é'));
        // A modifier-only keysym has no character.
        assert_eq!(char_of(0xffe1), None);
        assert_eq!(char_of(0), None);
    }
}

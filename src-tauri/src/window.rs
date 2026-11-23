//! Show/hide plumbing for the overlay window.
//!
//! The window is a frameless panel that starts hidden and is not on top unless
//! the user asks for it. Every entry point that reveals it (tray icon, global
//! hotkey, a command from the UI) goes through here so that focus and z-order
//! behave consistently.

use tauri::window::Color;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};

use crate::models::{ClipDetail, ClipKind};

/// Whether a window may ask for a transparent surface.
///
/// A transparent webview is an ARGB surface, and on X11 something has to blend it:
/// with no compositing manager running, the pixels WebKit never painted come out
/// as garbage, so the recording bar, the countdown and the frame would show
/// fragments of themselves. Off Windows, transparency is therefore only asked for
/// when the X server reports a compositor.
fn transparency_available() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        // Cocoa always composites, so the transparent surfaces the overlays and the
        // countdown ask for are blended there.
        true
    }
    #[cfg(target_os = "linux")]
    {
        // `_NET_WM_CM_S0` is the EWMH way to name the compositing manager for
        // screen 0; it is absent when nothing is blending.
        std::process::Command::new("xprop")
            .args(["-root", "_NET_WM_CM_S0"])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains("window id"))
            .unwrap_or(false)
    }
}

/// Must match `app.windows[0].label` in tauri.conf.json.
pub const WINDOW_LABEL: &str = "main";

/// Label of the separate settings window; also listed in the capability file.
pub const SETTINGS_LABEL: &str = "settings";

/// Label of the small "New clip" window; also listed in the capability file.
pub const ADD_CLIP_LABEL: &str = "add-clip";

/// Label of the setup wizard's own window; also listed in the capability file.
pub const WIZARD_LABEL: &str = "wizard";

/// Label of the update window that shows what a release contains; also listed in
/// the capability file.
pub const WHATS_NEW_LABEL: &str = "whats-new";

/// Prefix shared by the independent window for each sticky clip.
pub const STICKY_PREFIX: &str = "sticky-";

/// Label of the screen-recording toolbar and its floating controls; also listed in
/// the capability file.
pub const RECORD_BAR_LABEL: &str = "record-bar";

/// Label of the paste stack's own window; also listed in the capability file.
pub const STACK_LABEL: &str = "stack";

/// Label of the `::` auto-complete popup; also listed in the capability file.
/// Label of the `::` popup; also listed in the capability file.
pub const QUICK_LABEL: &str = "quick";

/// Width of the `::` popup, in logical pixels.
///
/// Fixed: it is a search field over a list of one-line rows, and a resizable
/// version of it would be a different feature.
pub const QUICK_WIDTH: f64 = 380.0;

/// Height of the popup as it opens — the search field and one row. The interface
/// grows the window as the list does; this is only what placement is decided on.
pub const QUICK_FIRST_HEIGHT: f64 = 92.0;

/// Label of the full-screen window that places a fixed-size recording frame.
pub const RECORD_AREA_LABEL: &str = "record-area";

/// Emitted to tell the frontend which pane should be in front. The main window
/// is reused rather than opening a second one for settings.
pub const EVENT_ROUTE: &str = "ui:route";

/// Emitted to the settings window when a command asks for one section by name.
pub const EVENT_SETTINGS_SECTION: &str = "settings:section";

pub const ROUTE_HISTORY: &str = "history";

pub fn window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(WINDOW_LABEL)
}

/// The color every solid window paints behind its page.
///
/// A webview paints nothing until its page has loaded, and the platform default
/// underneath is what flashes as a black box with a white inside when a window
/// opens. The theme's canvas color is what the page paints a moment later, so the
/// hand-off is invisible.
fn theme_background<R: Runtime>(app: &AppHandle<R>) -> Color {
    let canvas = app
        .state::<crate::AppState>()
        .store
        .settings()
        .theme
        .canvas();
    parse_hex_color(&canvas).unwrap_or(Color(11, 14, 20, 255))
}

fn parse_hex_color(value: &str) -> Option<Color> {
    let digits = value.trim().strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&digits[range], 16).ok();
    Some(Color(channel(0..2)?, channel(2..4)?, channel(4..6)?, 255))
}

/// Paints the theme's canvas color behind the page of every window open now.
///
/// The capture overlays are deliberately see-through and are left alone; painting
/// them would put a slab of the app's background behind a screenshot.
pub fn apply_theme_background<R: Runtime>(app: &AppHandle<R>) {
    let color = theme_background(app);
    for window in app.webview_windows().values() {
        match window.label() {
            RECORD_AREA_LABEL | RECORD_BAR_LABEL | COUNTDOWN_LABEL => continue,
            _ => {}
        }
        if let Err(e) = window.set_background_color(Some(color)) {
            log::debug!("could not paint the background of {}: {e}", window.label());
        }
    }
}

/// Raw handle of our own window, used to avoid mistaking it for "the app the user
/// came from" when deciding where an auto-paste should land.
#[cfg(windows)]
pub fn own_window_handle<R: Runtime>(app: &AppHandle<R>) -> Option<isize> {
    let window = window(app)?;
    window.hwnd().ok().map(|handle| handle.0 as isize)
}

#[cfg(not(windows))]
pub fn own_window_handle<R: Runtime>(_app: &AppHandle<R>) -> Option<isize> {
    None
}

pub fn show<R: Runtime>(app: &AppHandle<R>) {
    show_route(app, ROUTE_HISTORY);
}

pub fn show_settings<R: Runtime>(app: &AppHandle<R>) {
    show_settings_at(app, None);
}

/// Shows the settings window, optionally scrolled to one section.
///
/// The section is a slug understood by the settings page ("screen-recording"). When
/// the window is already open it is told where to go rather than being rebuilt, so
/// the page keeps whatever the reader had open.
pub fn show_settings_at<R: Runtime>(app: &AppHandle<R>, section: Option<&str>) {
    if let Some(existing) = app.get_webview_window(SETTINGS_LABEL) {
        if let Some(section) = section {
            let _ = existing.emit(EVENT_SETTINGS_SECTION, section);
        }
        let _ = existing.show();
        let _ = existing.set_focus();
        return;
    }
    // Webviews can only be created from the event loop thread, and the call
    // blocks until that thread is free, so it must not run on the caller's
    // thread when the caller is the tray menu or a command.
    let handle = app.clone();
    let section = section.map(str::to_string);
    if let Err(e) = std::thread::Builder::new()
        .name("settings-window".into())
        .spawn(move || {
            if let Err(e) = open_settings(&handle, section.as_deref()) {
                log::warn!("could not open the settings window: {e}");
            }
        })
    {
        log::warn!("could not spawn the settings window thread: {e}");
    }
}

/// Opens the settings page in its own decorated window.
///
/// A separate window keeps the overlay a pure history surface: the settings page
/// is long, has its own scrolling, and benefits from ordinary minimise and
/// maximise behaviour.
pub fn open_settings<R: Runtime>(app: &AppHandle<R>, section: Option<&str>) -> tauri::Result<()> {
    const LABEL: &str = SETTINGS_LABEL;
    if let Some(existing) = app.get_webview_window(LABEL) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }
    let window = tauri::WebviewWindowBuilder::new(
        app,
        LABEL,
        // The query is how the page knows it is the settings window. Asking Tauri
        // for the label works too, but only once its internals are installed, and a
        // webview that renders before that shows the history UI — the "settings
        // window became another main window" bug.
        tauri::WebviewUrl::App(match section {
            Some(section) => format!("index.html?window=settings&section={section}"),
            None => "index.html?window=settings".to_string(),
        }
        .into()),
    )
    .title("Kept Settings")
    .inner_size(780.0, 820.0)
    .min_inner_size(560.0, 480.0)
    // Painted before the page draws, so opening the window does not flash the
    // platform's default background first.
    .background_color(theme_background(app))
    .always_on_top(always_on_top(app))
    .center()
    .build()?;
    let _ = window.set_focus();
    Ok(())
}

/// Opens (or focuses) the setup wizard's own window.
///
/// The wizard used to be an overlay inside the history window, which made it a
/// modal that could not be moved off the clips it was asking about. Its own window
/// has a title bar to drag and a close button; minimising or maximising a
/// three-step form is not something anyone needs, so those two are switched off.
pub fn show_wizard<R: Runtime>(app: &AppHandle<R>) {
    if let Some(existing) = app.get_webview_window(WIZARD_LABEL) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return;
    }
    // Same constraint as the settings window: webviews have to be created from the
    // event loop thread, so the call cannot run on the caller's thread.
    let handle = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("wizard-window".into())
        .spawn(move || {
            if let Err(e) = open_wizard(&handle) {
                log::warn!("could not open the setup wizard: {e}");
            }
        })
    {
        log::warn!("could not spawn the wizard window thread: {e}");
    }
}

/// Opens the setup wizard in its own decorated window.
pub fn open_wizard<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    const LABEL: &str = WIZARD_LABEL;
    if let Some(existing) = app.get_webview_window(LABEL) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }
    let builder = tauri::WebviewWindowBuilder::new(
        app,
        LABEL,
        // Same reason as the settings window: say which window this is in the URL.
        tauri::WebviewUrl::App("index.html?window=wizard".into()),
    )
    .title("Set up Kept")
    .inner_size(560.0, 780.0)
    .min_inner_size(460.0, 480.0)
    .minimizable(false)
    .maximizable(false)
    .background_color(theme_background(app))
    .always_on_top(always_on_top(app))
    .center();
    // macOS paints its own light title bar above the page's dark header, leaving a
    // lone red button beside the page's own close control. There the header is the
    // title bar — it carries the drag handle — and the frame is dropped. Windows
    // keeps the frame it has always had.
    #[cfg(target_os = "macos")]
    let builder = builder.decorations(false);
    let window = builder.build()?;
    let _ = window.set_focus();
    Ok(())
}

/// Opens (or focuses) the window that shows what a released version contains.
///
/// It has to be a window of its own rather than an overlay: the release notes are
/// a page to read and act on, with the update buttons beside them.
pub fn show_whats_new<R: Runtime>(app: &AppHandle<R>) {
    if app.get_webview_window(WHATS_NEW_LABEL).is_some() {
        if let Err(e) = open_whats_new(app) {
            log::warn!("could not focus the update window: {e}");
        }
        return;
    }
    // Same constraint as the settings window: webviews have to be created from the
    // event loop thread, so the call cannot run on the caller's thread.
    let handle = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("whats-new-window".into())
        .spawn(move || {
            if let Err(e) = open_whats_new(&handle) {
                log::warn!("could not open the update window: {e}");
            }
        })
    {
        log::warn!("could not spawn the update window thread: {e}");
    }
}

/// Opens the update window: frameless, because its own header carries the drag
/// handle and the close button.
pub fn open_whats_new<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    const LABEL: &str = WHATS_NEW_LABEL;
    if let Some(existing) = app.get_webview_window(LABEL) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }
    let window = tauri::WebviewWindowBuilder::new(
        app,
        LABEL,
        // Same reason as the settings window: say which window this is in the URL.
        tauri::WebviewUrl::App("index.html?window=whats-new".into()),
    )
    .title("What's new in Kept")
    .inner_size(660.0, 760.0)
    .min_inner_size(520.0, 460.0)
    .decorations(false)
    .background_color(theme_background(app))
    .center()
    .build()?;
    let _ = window.set_focus();
    Ok(())
}

pub fn show_add_clip<R: Runtime>(app: &AppHandle<R>) {
    if app.get_webview_window(ADD_CLIP_LABEL).is_some() {
        if let Err(e) = open_add_clip(app) {
            log::warn!("could not focus the add-clip window: {e}");
        }
        return;
    }
    // Same constraint as the settings window: webviews have to be created from the
    // event loop thread, so the call cannot run on the caller's thread.
    let handle = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("add-clip-window".into())
        .spawn(move || {
            if let Err(e) = open_add_clip(&handle) {
                log::warn!("could not open the add-clip window: {e}");
            }
        })
    {
        log::warn!("could not spawn the add-clip window thread: {e}");
    }
}

/// Opens the "New clip" form in its own decorated window.
///
/// Typing a clip is a short, deliberate act, and it gets a compact window rather
/// than a pane in the overlay so the history stays visible behind it.
pub fn open_add_clip<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    const LABEL: &str = ADD_CLIP_LABEL;
    if let Some(existing) = app.get_webview_window(LABEL) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }
    let window = tauri::WebviewWindowBuilder::new(
        app,
        LABEL,
        // Same reason as the settings window: say which window this is in the URL.
        tauri::WebviewUrl::App("index.html?window=add-clip".into()),
    )
    .title("New clip")
    .inner_size(460.0, 420.0)
    .min_inner_size(360.0, 280.0)
    .background_color(theme_background(app))
    .always_on_top(always_on_top(app))
    .center()
    .build()?;
    let _ = window.set_focus();
    Ok(())
}

/// Opens (or focuses) the paste stack's own window.
///
/// Always on top and out of the taskbar: the whole point of popping the queue out
/// is to keep it beside the application being pasted into, without adding another
/// entry to the window list. Frameless, so it reads as a panel rather than a
/// second Kept; dragging it by its header is what `data-tauri-drag-region`
/// in the page is for.
pub fn show_stack_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(existing) = app.get_webview_window(STACK_LABEL) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return;
    }
    // Same reason as the settings window: a webview can only be built from the
    // event loop thread, and a window built from a command's own thread comes up
    // as a blank white rectangle.
    let handle = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("stack-window".into())
        .spawn(move || {
            if let Err(e) = open_stack_window(&handle) {
                log::warn!("could not open the stack window: {e}");
            }
        })
    {
        log::warn!("could not spawn the stack window thread: {e}");
    }
}

fn open_stack_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if app.get_webview_window(STACK_LABEL).is_some() {
        return Ok(());
    }
    let window = tauri::WebviewWindowBuilder::new(
        app,
        STACK_LABEL,
        // Same reason as the settings window: the URL says which window this is.
        tauri::WebviewUrl::App("index.html?window=stack".into()),
    )
    .title("Paste stack")
    .inner_size(320.0, 460.0)
    .min_inner_size(260.0, 240.0)
    .background_color(theme_background(app))
    .always_on_top(true)
    .decorations(false)
    .skip_taskbar(true)
    .build()?;
    let _ = window.set_focus();
    Ok(())
}

/// Closes the stack window.
///
/// Closing it is not a request to reopen the panel in the main window: the user
/// dismissed the queue, and the next time they want it they ask for it. The queue
/// itself is untouched — it lives in the database.
pub fn close_stack_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(STACK_LABEL) {
        window.close()?;
    }
    Ok(())
}

/// True while the `::` popup is on screen.
///
/// The trigger asks this before doing anything else: a second token typed while the
/// popup is up would otherwise replace the list the user is typing into.
pub fn quick_is_open<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.get_webview_window(QUICK_LABEL).is_some()
}

/// Closes the `::` popup.
///
/// It is destroyed rather than hidden, and rebuilt for the next trigger: the popup
/// is a fresh search every time, and a leftover window would have to be reset by
/// hand.
pub fn close_quick<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(QUICK_LABEL) {
        // `destroy` skips the close-request handshake and tears the webview down
        // directly. The popup has nothing to save, and `close` can be swallowed
        // while the page is still starting up, which leaves the box on screen.
        if let Err(e) = window.destroy() {
            log::warn!("could not close the quick popup: {e}");
        }
    }
}

/// Shows the `::` popup under `caret`, or under the pointer when the application
/// being typed into does not report a caret.
pub fn show_quick<R: Runtime>(app: &AppHandle<R>, caret: Option<(i32, i32)>) {
    if let Some(existing) = app.get_webview_window(QUICK_LABEL) {
        // A leftover window from a previous trigger is closed rather than shown
        // again: it would still hold the old search.
        let _ = existing.destroy();
    }
    let origin = quick_origin(caret, window_scale(app));
    if let Err(e) = open_quick(app, origin) {
        log::warn!("could not open the quick popup: {e}");
    }
}

fn open_quick<R: Runtime>(app: &AppHandle<R>, origin: (i32, i32)) -> tauri::Result<()> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        QUICK_LABEL,
        tauri::WebviewUrl::App("index.html?window=quick".into()),
    )
    .title("Kept quick paste")
    // One row and the search field to begin with; the interface grows it as the
    // list does.
    .inner_size(QUICK_WIDTH, QUICK_FIRST_HEIGHT)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .background_color(theme_background(app))
    .visible(false)
    .build()?;
    // Placed physically rather than through the builder: the caret arrives in
    // physical pixels from an application that may be on a differently scaled
    // monitor than the one this window would be measured against.
    let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(
        origin.0, origin.1,
    )));
    let _ = window.show();
    // Shown is not focused: the keystroke that asked for this window went to the
    // application being typed into, so Windows refuses the ordinary focus call and
    // the popup would sit there without the keyboard.
    #[cfg(windows)]
    if let Ok(handle) = window.hwnd() {
        if !crate::paste::take_foreground(handle.0 as isize) {
            log::warn!("could not take the foreground for the quick popup");
        }
    }
    let _ = window.set_focus();

    // Losing the focus is a dismissal, so it is enforced here as well as in the
    // page: the popup belongs to the application it was opened over, and anything
    // that takes the attention away means the user has moved on. A webview that is
    // busy — mid-commit, or waiting on a slow backend call — cannot be relied on to
    // report its own blur, which is how the menu used to be left behind.
    {
        let app = app.clone();
        let was_focused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        window.on_window_event(move |event| {
            let tauri::WindowEvent::Focused(focused) = event else {
                return;
            };
            if *focused {
                was_focused.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
            // A blur before the window was ever activated is the platform catching
            // up with the window's own creation, not the user looking elsewhere.
            if !was_focused.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            // Closed from another thread, and only after a beat: the platform
            // sometimes reports a blur immediately after the activation it just
            // gave us, and a menu that vanished on its own would be worse than one
            // that takes a moment to go.
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let still_ours = app
                    .get_webview_window(QUICK_LABEL)
                    .and_then(|window| window.is_focused().ok())
                    .unwrap_or(false);
                if !still_ours {
                    close_quick(&app);
                }
            });
        });
    }
    Ok(())
}

/// Where the popup's top-left corner goes, in physical screen pixels.
///
/// It hangs just under the caret, and sits above it when the bottom of the monitor
/// is close. Only the height it opens at is used to decide: the box grows downward
/// from there, so a long list covers the caret rather than floating a hundred
/// pixels away from it.
fn quick_origin(caret: Option<(i32, i32)>, scale: f64) -> (i32, i32) {
    let Some(point) = caret.or_else(cursor_point) else {
        return (200, 200);
    };
    let width = (QUICK_WIDTH * scale).round() as i32;
    let first = (QUICK_FIRST_HEIGHT * scale).round() as i32;
    let gap = (18.0 * scale).round() as i32;
    let Some((left, top, right, bottom)) = work_area(point) else {
        return (point.0, point.1 + gap);
    };

    let x = point.0.clamp(left, (right - width).max(left));
    let below = point.1 + gap;
    let y = if below + first <= bottom {
        below
    } else {
        (point.1 - first - gap).max(top)
    };
    (x, y)
}

fn window_scale<R: Runtime>(app: &AppHandle<R>) -> f64 {
    window(app)
        .and_then(|window| window.scale_factor().ok())
        .unwrap_or(1.0)
}

#[cfg(windows)]
fn cursor_point() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut point = POINT::default();
    // SAFETY: GetCursorPos only writes to the point it is given.
    unsafe { GetCursorPos(&mut point).is_ok().then_some((point.x, point.y)) }
}

#[cfg(not(windows))]
fn cursor_point() -> Option<(i32, i32)> {
    None
}

/// Work area of the monitor `point` is on, as `(left, top, right, bottom)`.
#[cfg(windows)]
fn work_area(point: (i32, i32)) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };

    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: the monitor handle is only used for the length of this call, and the
    // struct is the size the call is told to expect.
    unsafe {
        let monitor = MonitorFromPoint(
            POINT {
                x: point.0,
                y: point.1,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        if monitor.is_invalid() || !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
    }
    let work = info.rcWork;
    Some((work.left, work.top, work.right, work.bottom))
}

#[cfg(not(windows))]
fn work_area(_point: (i32, i32)) -> Option<(i32, i32, i32, i32)> {
    None
}

pub fn show_sticky_clip<R: Runtime>(app: &AppHandle<R>, id: i64, size: (f64, f64)) {    let label = format!("{STICKY_PREFIX}{id}");
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return;
    }

    let handle = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name(format!("sticky-clip-{id}"))
        .spawn(move || {
            if let Err(e) = open_sticky_clip(&handle, id, size) {
                log::warn!("could not open sticky clip {id}: {e}");
            }
        })
    {
        log::warn!("could not spawn sticky clip {id} window: {e}");
    }
}

fn open_sticky_clip<R: Runtime>(
    app: &AppHandle<R>,
    id: i64,
    size: (f64, f64),
) -> tauri::Result<()> {
    let label = format!("{STICKY_PREFIX}{id}");
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }
    let window = tauri::WebviewWindowBuilder::new(
        app,
        label,
        tauri::WebviewUrl::App(format!("index.html?window=sticky&id={id}").into()),
    )
    .title("Sticky clip")
    .inner_size(size.0, size.1)
    .min_inner_size(220.0, 160.0)
    .resizable(true)
    .decorations(false)
    .background_color(theme_background(app))
    .always_on_top(true)
    .build()?;
    let _ = window.set_focus();
    Ok(())
}

/// Shows the screen-recording bar: the mode toolbar before a recording starts, the
/// pause and stop controls while one runs.
///
/// It is frameless, always on top and parked at the top of the screen, so the app
/// can be minimised for the recording and its controls still be reachable.
pub fn show_record_bar<R: Runtime>(app: &AppHandle<R>) {
    if let Some(existing) = app.get_webview_window(RECORD_BAR_LABEL) {
        let _ = existing.show();
        return;
    }
    // Webviews can only be built from the event loop thread, and the call blocks
    // until that thread is free, so a command cannot make it on its own thread.
    let handle = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("record-bar-window".into())
        .spawn(move || {
            if let Err(e) = open_record_bar(&handle) {
                log::warn!("could not open the recording bar: {e}");
            }
        })
    {
        log::warn!("could not spawn the recording bar thread: {e}");
    }
}

fn open_record_bar<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        RECORD_BAR_LABEL,
        tauri::WebviewUrl::App("index.html?window=record-bar".into()),
    )
    .title("Record")
    // Sized for the idle toolbar; the frontend resizes it to fit the rows it shows.
    .inner_size(488.0, 100.0)
    .resizable(false)
    .decorations(false)
    .transparent(transparency_available())
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    // Excluded from screen capture (WDA_EXCLUDEFROMCAPTURE), so the timer and the
    // buttons never end up in the recording they are controlling.
    .content_protected(true)
    .build()?;
    // Top centre of the monitor the window opened on: clear of title bars and of
    // the menus that sit under a title bar.
    if let Ok(Some(monitor)) = window.current_monitor() {
        let size = monitor.size();
        let origin = monitor.position();
        let scale = monitor.scale_factor();
        let width = window
            .outer_size()
            .map(|size| size.width as f64)
            .unwrap_or(360.0 * scale);
        let x = origin.x + ((size.width as f64 - width) / 2.0).max(0.0) as i32;
        let y = origin.y + (18.0 * scale) as i32;
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
    let _ = window.show();
    Ok(())
}

/// Opens the full-screen placement overlay for a fixed-size recording.
///
/// It covers the virtual desktop, is transparent and content-protected, and the
/// frontend draws the frame and its record button. Closing it (Esc) leaves the
/// recording bar underneath untouched.
pub fn show_record_area<R: Runtime>(app: &AppHandle<R>, width: i32, height: i32) {
    if let Some(existing) = app.get_webview_window(RECORD_AREA_LABEL) {
        let _ = existing.close();
    }
    let handle = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("record-area-window".into())
        .spawn(move || {
            if let Err(e) = open_record_area(&handle, width, height) {
                log::warn!("could not open the recording frame: {e}");
            }
        })
    {
        log::warn!("could not spawn the recording frame thread: {e}");
    }
}

fn open_record_area<R: Runtime>(
    app: &AppHandle<R>,
    width: i32,
    height: i32,
) -> tauri::Result<()> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        RECORD_AREA_LABEL,
        tauri::WebviewUrl::App(
            format!("index.html?window=record-area&width={width}&height={height}").into(),
        ),
    )
    .title("Place the recording frame")
    .inner_size(640.0, 480.0)
    .resizable(false)
    .decorations(false)
    .transparent(transparency_available())
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(true)
    .content_protected(true)
    .shadow(false)
    .visible(false)
    .build()?;
    #[cfg(windows)]
    {
        let bounds = crate::capture::virtual_bounds();
        let _ = window.set_position(tauri::PhysicalPosition::new(bounds.left, bounds.top));
        let _ = window.set_size(tauri::PhysicalSize::new(
            bounds.width().max(1) as u32,
            bounds.height().max(1) as u32,
        ));
    }
    #[cfg(not(windows))]
    {
        let _ = window.set_fullscreen(true);
    }
    let _ = window.show();
    let _ = window.set_focus();
    Ok(())
}

/// Closes the bar, e.g. once a recording has been stopped or the encoder is missing.
pub fn close_record_bar<R: Runtime>(app: &AppHandle<R>) {
    match app.get_webview_window(RECORD_BAR_LABEL) {
        Some(window) => match window.close() {
            Ok(()) => log::info!("closed the recording bar"),
            Err(e) => log::warn!("could not close the recording bar: {e}"),
        },
        None => log::info!("the recording bar was already closed"),
    }
}

/// Label of the countdown shown before a delayed screenshot is taken.
pub const COUNTDOWN_LABEL: &str = "countdown";

/// Title of the countdown window.
///
/// X11 has no other handle on a window Tauri built, so the title is also how the
/// shaping below finds it — which is why it is a constant rather than a literal.
const COUNTDOWN_TITLE: &str = "Screenshot delay";

/// Shows the countdown for a delayed screenshot, in the bottom right corner of the
/// primary monitor.
///
/// It is a window of its own rather than part of the capture overlay: the area is
/// chosen before it appears, the desktop stays usable while it counts, and it has
/// to be gone by the time the pixels are read so the number cannot end up in the
/// picture it is counting down to.
pub fn show_countdown<R: Runtime>(app: &AppHandle<R>, seconds: u32) {
    close_countdown(app);
    let handle = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("capture-countdown".into())
        .spawn(move || {
            if let Err(e) = open_countdown(&handle, seconds) {
                log::warn!("could not open the capture countdown: {e}");
            }
        })
    {
        log::warn!("could not spawn the capture countdown thread: {e}");
    }
}

fn open_countdown<R: Runtime>(app: &AppHandle<R>, seconds: u32) -> tauri::Result<()> {
    let side = 240.0;
    let window = tauri::WebviewWindowBuilder::new(
        app,
        COUNTDOWN_LABEL,
        tauri::WebviewUrl::App(format!("index.html?window=countdown&seconds={seconds}").into()),
    )
    .title(COUNTDOWN_TITLE)
    .inner_size(side, side)
    .resizable(false)
    .decorations(false)
    .transparent(transparency_available())
    .always_on_top(true)
    .skip_taskbar(true)
    // Deliberately not focused: the countdown runs while the user is still using
    // the desktop, so it must not take the keyboard away from whatever they are
    // typing into. Esc is caught system-wide instead (see `countdown`), so
    // cancelling does not depend on this window having focus.
    .focused(false)
    .content_protected(true)
    .shadow(false)
    .visible(false)
    .build()?;
    #[cfg(windows)]
    {
        // Bottom right of the primary monitor, out of the way of the middle of the
        // screen, which is usually what the screenshot is about to be taken of.
        let bounds = crate::capture::primary_bounds();
        let outer = window
            .outer_size()
            .unwrap_or(tauri::PhysicalSize::new(side as u32, side as u32));
        let margin = 24;
        let x = bounds.left + (bounds.width() - outer.width as i32 - margin).max(0);
        let y = bounds.top + (bounds.height() - outer.height as i32 - margin).max(0);
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
    let _ = window.show();
    // The countdown is a ring drawn in a square window. With a compositor the
    // corners outside it are transparent; on a bare X session there is nothing to
    // blend them, and the whole square shows. Cutting the window itself down to the
    // ring leaves the desktop visible in the corners either way.
    #[cfg(target_os = "linux")]
    if !transparency_available() {
        round_window(COUNTDOWN_TITLE);
    }
    Ok(())
}

/// Cuts a window down to the circle its page draws, so its corners are not part of
/// it at all.
///
/// Only worth doing where transparency is unavailable: with a compositor the ARGB
/// corners are already smooth, and a hard X11 shape would only take the
/// antialiasing away.
#[cfg(target_os = "linux")]
fn round_window(title: &str) {
    match cut_to_circle(title) {
        Ok(()) => log::debug!("rounded the {title} window off"),
        // Only a cosmetic touch, so a server without SHAPE loses nothing else.
        Err(e) => log::debug!("could not round the {title} window: {e}"),
    }
}

/// The SHAPE work behind [`round_window`]: a one-bit mask holding a filled circle
/// becomes the window's boundary.
#[cfg(target_os = "linux")]
fn cut_to_circle(title: &str) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::shape::{self, ConnectionExt as _};
    use x11rb::protocol::xproto::{Arc, ConnectionExt as _, CreateGCAux};

    // Tauri hands out no X11 id, so the window is found by the title it was given;
    // the countdown is the only window with this one, and it was built just now.
    let id = window_id_by_title(title)?;
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let geometry = conn.get_geometry(id)?.reply()?;
    let side = geometry.width.min(geometry.height);
    if side == 0 {
        return Ok(());
    }

    let mask = conn.generate_id()?;
    conn.create_pixmap(1, mask, root, side, side)?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, mask, &CreateGCAux::new().foreground(1u32).background(0u32))?;
    conn.poly_fill_arc(
        mask,
        gc,
        &[Arc {
            x: 0,
            y: 0,
            width: side,
            height: side,
            angle1: 0,
            // A full turn is 360° in units of 1/64°.
            angle2: 360 * 64,
        }],
    )?;
    conn.shape_mask(shape::SO::SET, shape::SK::BOUNDING, id, 0, 0, mask)?;
    conn.free_gc(gc)?;
    conn.free_pixmap(mask)?;
    conn.flush()?;
    Ok(())
}

/// The X11 id of the window with exactly `title`, waiting for it to be created and
/// mapped. Searched through the window tree so it does not depend on `xdotool` or on
/// the window manager reparenting it under a frame.
#[cfg(target_os = "linux")]
fn window_id_by_title(title: &str) -> Result<u32, Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;

    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let net_wm_name = conn
        .intern_atom(false, b"_NET_WM_NAME")?
        .reply()?
        .atom;
    let utf8_string = conn.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;

    for _ in 0..80 {
        if let Some(id) = find_window_by_title(&conn, root, title, net_wm_name, utf8_string)? {
            return Ok(id);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Err(format!("no window titled {title}").into())
}

/// Walks the window tree from `window` and returns the id of a window whose name is
/// exactly `title`.
#[cfg(target_os = "linux")]
fn find_window_by_title(
    conn: &impl x11rb::connection::Connection,
    window: u32,
    title: &str,
    net_wm_name: u32,
    utf8_string: u32,
) -> Result<Option<u32>, Box<dyn std::error::Error>> {
    use x11rb::protocol::xproto::ConnectionExt as _;

    if window_name_matches(conn, window, title, net_wm_name, utf8_string)? {
        return Ok(Some(window));
    }
    let tree = match conn.query_tree(window) {
        Ok(cookie) => match cookie.reply() {
            Ok(tree) => tree,
            Err(_) => return Ok(None),
        },
        Err(_) => return Ok(None),
    };
    for child in tree.children {
        if let Some(id) = find_window_by_title(conn, child, title, net_wm_name, utf8_string)? {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// True when the X11 window's name is exactly `title`.
#[cfg(target_os = "linux")]
fn window_name_matches(
    conn: &impl x11rb::connection::Connection,
    window: u32,
    title: &str,
    net_wm_name: u32,
    utf8_string: u32,
) -> Result<bool, Box<dyn std::error::Error>> {
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

    // Prefer _NET_WM_NAME (UTF-8); fall back to WM_NAME.
    let name = conn
        .get_property(false, window, net_wm_name, utf8_string, 0, 128)?
        .reply()?;
    if name.format == 8 && !name.value.is_empty() {
        return Ok(String::from_utf8_lossy(&name.value).trim_end_matches('\0') == title);
    }
    let name = conn
        .get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 128)?
        .reply()?;
    if name.format == 8 && !name.value.is_empty() {
        return Ok(String::from_utf8_lossy(&name.value).trim_end_matches('\0') == title);
    }
    Ok(false)
}

/// Takes the countdown down, whether it ran out or was cancelled.
pub fn close_countdown<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(COUNTDOWN_LABEL) {
        let _ = window.close();
    }
}

pub fn sticky_clip_size(detail: &ClipDetail) -> (f64, f64) {
    const MAX_WIDTH: f64 = 1_000.0;
    const MAX_HEIGHT: f64 = 800.0;
    const HEADER_HEIGHT: f64 = 36.0;

    match detail.summary.kind {
        ClipKind::Image => {
            let width = detail.summary.width.unwrap_or(480).max(1) as f64;
            let height = detail.summary.height.unwrap_or(320).max(1) as f64;
            let scale = 1.0_f64
                .min((MAX_WIDTH - 24.0) / width)
                .min((MAX_HEIGHT - HEADER_HEIGHT - 24.0) / height);
            (width * scale + 24.0, height * scale + HEADER_HEIGHT + 24.0)
        }
        ClipKind::Text => {
            let text = detail.text.as_deref().unwrap_or(&detail.summary.preview);
            let lines = text.lines().count().max(1);
            let longest_line = text
                .lines()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(1);
            (
                (longest_line as f64 * 8.0 + 32.0).clamp(320.0, MAX_WIDTH),
                (lines as f64 * 20.0 + HEADER_HEIGHT + 24.0).clamp(180.0, MAX_HEIGHT),
            )
        }
        ClipKind::Files => {
            let longest_path = detail
                .file_paths
                .iter()
                .map(|path| path.chars().count())
                .max()
                .unwrap_or(32);
            (
                (longest_path as f64 * 8.0 + 32.0).clamp(360.0, MAX_WIDTH),
                (detail.file_paths.len() as f64 * 22.0 + HEADER_HEIGHT + 24.0)
                    .clamp(180.0, MAX_HEIGHT),
            )
        }
    }
}

fn always_on_top<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.state::<crate::AppState>()
        .store
        .settings()
        .always_on_top
}

pub fn show_route<R: Runtime>(app: &AppHandle<R>, route: &'static str) {
    if let Some(window) = window(app) {
        // Closing a minimised window hides it in that state; showing it again has
        // to restore the size it had before it was minimised.
        if window.is_minimized().unwrap_or(false) {
            let _ = window.unminimize();
        }
        if let Err(e) = window.show() {
            log::warn!("could not show the window: {e}");
        }
        // Focus has to follow the show, otherwise the window appears without
        // keyboard focus and the search box cannot be typed into.
        if let Err(e) = window.set_focus() {
            log::warn!("could not focus the window: {e}");
        }
        // Addressed to this window only. `emit` would broadcast to every webview
        // in the app, and the settings window — which loads the same page and
        // listens for the same event — would switch itself to the history UI. That
        // is what made it "turn into another main window" after a screenshot:
        // restoring the main window after a capture is a route change like any
        // other.
        let _ = window.emit_to(window.label(), EVENT_ROUTE, route);
    }
}

pub fn hide<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = window(app)
        && let Err(e) = window.hide()
    {
        log::warn!("could not hide the window: {e}");
    }
}

/// Shows or hides the taskbar button. `skipTaskbar` is the inverse of the setting,
/// which is why this takes the user-facing value rather than the flag.
pub fn set_taskbar_visibility<R: Runtime>(app: &AppHandle<R>, show: bool) {
    if let Some(window) = window(app)
        && let Err(e) = window.set_skip_taskbar(!show)
    {
        log::warn!("could not change taskbar visibility: {e}");
    }
}

/// Applies the user's z-order preference to app windows; sticky clips stay on top.
pub fn set_always_on_top<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    for (label, window) in app.webview_windows() {
        let keep_on_top = enabled || label.starts_with(STICKY_PREFIX);
        if let Err(e) = window.set_always_on_top(keep_on_top) {
            log::warn!("could not change always-on-top for {label}: {e}");
        }
    }
}

pub fn is_visible<R: Runtime>(app: &AppHandle<R>) -> bool {
    window(app)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

/// Shows the window if hidden, hides it if visible.
pub fn toggle<R: Runtime>(app: &AppHandle<R>) {
    // A minimised window reports itself as visible, so treating "visible" as
    // "already on screen" would hide it instead of restoring it from the taskbar.
    let minimized = window(app)
        .and_then(|window| window.is_minimized().ok())
        .unwrap_or(false);
    if is_visible(app) && !minimized {
        hide(app);
    } else {
        show(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ClipKind, ClipSummary};

    #[test]
    fn hex_colors_parse_into_channels() {
        assert_eq!(parse_hex_color("#0b0e14"), Some(Color(11, 14, 20, 255)));
        assert_eq!(parse_hex_color("#eef1f6"), Some(Color(238, 241, 246, 255)));
        assert_eq!(parse_hex_color("0b0e14"), None, "the hash is required");
        assert_eq!(parse_hex_color("#fff"), None, "only six digits are accepted");
    }

    #[test]
    fn the_window_background_follows_the_theme_mode() {
        let dark = crate::settings::Theme::default();
        assert_eq!(dark.canvas(), "#0b0e14");
        let light = crate::settings::Theme {
            mode: "light".into(),
            ..Default::default()
        };
        assert_eq!(light.canvas(), "#eef1f6");
        let mut custom = crate::settings::Theme {
            mode: "custom".into(),
            ..Default::default()
        };
        custom.colors.canvas = "#123456".into();
        assert_eq!(custom.canvas(), "#123456");
    }

    fn detail(kind: ClipKind, width: Option<i64>, height: Option<i64>) -> ClipDetail {
        ClipDetail {
            summary: ClipSummary {
                id: 1,
                kind,
                preview: String::new(),
                byte_size: 0,
                source_app: None,
                pinned: false,
                created_at: 0,
                updated_at: 0,
                copy_count: 0,
                width,
                height,
                thumb_file: None,
                image_file: None,
                media_locked: false,
                file_count: 0,
                char_count: 0,
                split_item_count: 0,
                has_ocr: false,
                has_note: false,
                color: None,
                content_kind: None,
                image_path: None,
                animated: false,
                video: false,
                motion_pending: false,
                video_url: None,
                collection_ids: Vec::new(),
                tags: Vec::new(),
                secret_level: None,
                face_count: 0,
                url_cleaned: false,
                has_rich: false,
            },
            text: None,
            file_paths: Vec::new(),
            ocr_text: None,
            note: None,
            uncleaned_url: None,
        }
    }

    #[test]
    fn large_sticky_images_fit_within_window_bounds() {
        let size = sticky_clip_size(&detail(ClipKind::Image, Some(1920), Some(1080)));
        assert!(size.0 <= 1_000.0);
        assert!(size.1 <= 800.0);
    }

    #[test]
    fn text_sticky_windows_grow_to_fit_content() {
        let mut clip = detail(ClipKind::Text, None, None);
        clip.text = Some(format!(
            "{}\nsecond\nthird\nfourth\nfifth\nsixth\nseventh\neighth",
            "x".repeat(80)
        ));

        let size = sticky_clip_size(&clip);

        assert!(size.0 > 320.0);
        assert!(size.1 > 180.0);
    }

    #[test]
    fn file_sticky_windows_grow_to_fit_file_lists() {
        let mut clip = detail(ClipKind::Files, None, None);
        clip.file_paths = vec![format!(r"C:\{}", "x".repeat(80)); 8];

        let size = sticky_clip_size(&clip);

        assert!(size.0 > 360.0);
        assert!(size.1 > 180.0);
    }
}

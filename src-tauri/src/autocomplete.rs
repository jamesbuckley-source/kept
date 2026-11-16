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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::paste;
use crate::settings::{Settings, MAX_TOKEN_CHARS};
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

#[cfg(not(windows))]
pub fn apply(_app: &AppHandle, _settings: &Settings) {
    // Nothing to install: the trigger is a Win32 keyboard hook, and the other
    // platforms have no equivalent that this app has taken on yet.
    invalidate_foreground();
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
}

/// The hook's own state: the thread whose message loop has to be ended.
#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
struct Hook {
    thread: u32,
}

#[cfg(windows)]
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

#[cfg(not(windows))]
fn invalidate_foreground() {}

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

#[cfg(windows)]
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
}

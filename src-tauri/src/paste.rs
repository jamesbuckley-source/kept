//! Auto-paste: return focus to where the user was, then synthesise Ctrl+V.
//!
//! Two things have to happen before the keystroke is useful. First, our own
//! window must be out of the way, otherwise the paste lands in the search box the
//! user was just typing in. Second, the previous window has to actually be
//! foreground again — Windows only honours synthetic input for the focused
//! window, so `SetForegroundWindow` is a prerequisite rather than a nicety.
//!
//! This is opt-in and off by default: injecting keystrokes into other
//! applications is exactly the kind of behaviour a user should choose.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;

use crate::error::{AppError, Result};

#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput, VIRTUAL_KEY, VK_CONTROL,
};

/// Window handle to paste into, captured when the history window is summoned.
/// `0` means "nothing remembered".
static TARGET: AtomicIsize = AtomicIsize::new(0);

pub fn target() -> Option<isize> {
    match TARGET.load(Ordering::Relaxed) {
        0 => None,
        handle => Some(handle),
    }
}

/// X11 has no window handles to hold onto, so the port drives `xdotool` for the
/// few things Windows does through `user32`.
#[cfg(target_os = "linux")]
pub(crate) fn xdotool(args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("xdotool")
        .args(args)
        .output()
        .map_err(|e| AppError::msg(format!("xdotool is not available ({e}); install xdotool")))?;
    if !output.status.success() {
        return Err(AppError::msg(format!(
            "xdotool {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The X11 window that owns the input focus, if there is one.
#[cfg(target_os = "linux")]
fn active_window() -> Option<isize> {
    xdotool(&["getactivewindow"])
        .ok()?
        .trim()
        .parse::<isize>()
        .ok()
        .filter(|id| *id != 0)
}

/// Records the window that is in front right now, ignoring `own_window`.
///
/// Called as the hotkey fires, before our window is shown, so the remembered
/// handle is the application the user was working in.
#[cfg(windows)]
pub fn capture_foreground(own_window: Option<isize>) {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: GetForegroundWindow takes no arguments and cannot fail.
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return;
    }
    if Some(window.0 as isize) == own_window {
        return;
    }
    TARGET.store(window.0 as isize, Ordering::Relaxed);
}

/// Records the application in front, which is what a paste reactivates.
///
/// macOS hands out a process id rather than a window: reactivating an application
/// brings back the window the user was working in, and `CGEventPost` types into
/// whatever is frontmost when the keystroke arrives.
#[cfg(target_os = "macos")]
pub fn capture_foreground(_own_window: Option<isize>) {
    if let Some(target) = frontmost_target(Some(std::process::id() as isize)) {
        TARGET.store(target, Ordering::Relaxed);
    }
}

/// The frontmost process id, unless it is one of ours.
#[cfg(target_os = "macos")]
fn frontmost_target(own: impl IntoIterator<Item = isize>) -> Option<isize> {
    let pid = crate::macos_clipboard::frontmost_pid()? as isize;
    // Our own process in front means the user is inside Kept already, and pasting
    // into our own window would be worse than leaving the last target alone.
    (!own.into_iter().any(|own| own == pid)).then_some(pid)
}

/// The X11 focus owner stands in for Windows' foreground window.
#[cfg(target_os = "linux")]
pub fn capture_foreground(_own_window: Option<isize>) {
    if let Some(window) = active_window() {
        TARGET.store(window, Ordering::Relaxed);
    }
}

/// Records the window in front, ignoring every one of our own windows.
///
/// The stack's shortcut is pressed while another application has the focus, so
/// there is no "our window" to exclude in the usual sense — but the settings or
/// preview window can be the one in front, and pasting into our own interface
/// would be worse than doing nothing. An own window in front leaves the remembered
/// target alone.
#[cfg(windows)]
pub fn capture_foreground_excluding(own: &[isize]) {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: GetForegroundWindow takes no arguments and cannot fail.
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() || own.contains(&(window.0 as isize)) {
        return;
    }
    TARGET.store(window.0 as isize, Ordering::Relaxed);
}

#[cfg(target_os = "macos")]
pub fn capture_foreground_excluding(own: &[isize]) {
    let mut excluded = own.to_vec();
    excluded.push(std::process::id() as isize);
    if let Some(target) = frontmost_target(excluded) {
        TARGET.store(target, Ordering::Relaxed);
    }
}

#[cfg(target_os = "linux")]
pub fn capture_foreground_excluding(_own: &[isize]) {
    if let Some(window) = active_window() {
        TARGET.store(window, Ordering::Relaxed);
    }
}

/// Brings a remembered window back to the foreground, restoring it if minimised.
#[cfg(windows)]
pub fn activate(handle: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsIconic, IsWindow, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };

    let window = HWND(handle as *mut core::ffi::c_void);
    // SAFETY: `window` comes from GetForegroundWindow, and every call below only
    // reads or adjusts that window. IsWindow guards against it having been closed
    // while the history window was open.
    unsafe {
        if !IsWindow(Some(window)).as_bool() {
            return false;
        }
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        SetForegroundWindow(window).as_bool()
    }
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
pub fn activate(handle: isize) -> bool {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};

    // A process id rather than a window: reactivating the application puts back
    // whichever window it was showing.
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(handle as i32)
    else {
        return false;
    };
    // The ignoring-other-apps flag is deprecated in macOS 14, where activation
    // became cooperative and applications are expected to ask rather than take —
    // but Kept is the frontmost application at this moment, so the request is
    // granted, and the flag is still what macOS 11 through 13 need.
    app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps)
}

#[cfg(target_os = "linux")]
pub fn activate(handle: isize) -> bool {
    xdotool(&["windowactivate", "--sync", &handle.to_string()]).is_ok()
}

/// True when `handle` is the window Windows is currently sending input to.
#[cfg(windows)]
fn foreground_is(handle: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: GetForegroundWindow takes no arguments and cannot fail.
    unsafe { GetForegroundWindow().0 as isize == handle }
}

#[cfg(target_os = "macos")]
fn foreground_is(handle: isize) -> bool {
    crate::macos_clipboard::frontmost_pid() == Some(handle as i32)
}

#[cfg(target_os = "linux")]
fn foreground_is(handle: isize) -> bool {
    active_window() == Some(handle)
}

/// Hides our window, focuses the target, and sends Ctrl+V.
pub fn restore_and_paste(handle: isize, delay_ms: u64) -> Result<()> {
    let delay = Duration::from_millis(delay_ms.min(2_000));
    std::thread::sleep(delay);

    // A stack paste is fired while the target already has the focus, so there is
    // nothing to activate — and `SetForegroundWindow` is refused for a process
    // that does not own the foreground, which is exactly our position there.
    // Only a paste that follows our window appearing needs the handover.
    //
    // X11 asks anyway: the focus a window manager reports is the focus it has
    // settled on, and the window that just closed leaves the answer stale for a
    // moment, so the activation is what makes the keystroke land where the user
    // was typing rather than wherever the last window was.
    //
    // macOS asks only when the target is not already in front: activation is
    // cooperative there, and re-activating the application that is already active
    // is a needless report of failure if AppKit declines to answer.
    #[cfg(target_os = "linux")]
    let needs_activation = true;
    #[cfg(not(target_os = "linux"))]
    let needs_activation = !foreground_is(handle);
    if needs_activation {
        if !activate(handle) {
            return Err(AppError::msg(
                "the window you copied from is no longer available",
            ));
        }

        // Give the window manager a moment to finish the activation before the
        // keystroke arrives; without this the input can be discarded or routed to
        // the window that is still in the process of losing focus.
        std::thread::sleep(Duration::from_millis(if cfg!(windows) { 40 } else { 180 }));
    }
    // The stack's own shortcut is a chord — Alt+Shift+V — and the user is often
    // still holding it when the paste lands, which turns Ctrl+V into
    // Ctrl+Shift+V and pastes nothing (or the wrong thing) in plenty of editors.
    clear_modifiers();
    send_paste()
}

/// Releases Shift, Ctrl and Alt, so the chord that asked for the paste cannot
/// modify it. Releasing a key that is not down is a no-op.
#[cfg(windows)]
fn clear_modifiers() {
    const VK_LSHIFT: u16 = 0xA0;
    const VK_RSHIFT: u16 = 0xA1;
    const VK_LCONTROL: u16 = 0xA2;
    const VK_RCONTROL: u16 = 0xA3;
    const VK_LMENU: u16 = 0xA4;
    const VK_RMENU: u16 = 0xA5;

    let inputs = [
        key_input(VIRTUAL_KEY(VK_LSHIFT), true),
        key_input(VIRTUAL_KEY(VK_RSHIFT), true),
        key_input(VIRTUAL_KEY(VK_LCONTROL), true),
        key_input(VIRTUAL_KEY(VK_RCONTROL), true),
        key_input(VIRTUAL_KEY(VK_LMENU), true),
        key_input(VIRTUAL_KEY(VK_RMENU), true),
    ];
    // SAFETY: a valid, correctly sized slice of INPUT structures.
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Releases Shift, Control, Option and Command, so a held chord cannot modify
/// the paste. A key-up for a key that is not down is a no-op.
#[cfg(target_os = "macos")]
fn clear_modifiers() {
    for key in [
        cg::KEY_COMMAND,
        cg::KEY_SHIFT,
        cg::KEY_RIGHT_SHIFT,
        cg::KEY_CONTROL,
        cg::KEY_RIGHT_CONTROL,
        cg::KEY_OPTION,
        cg::KEY_RIGHT_OPTION,
    ] {
        cg::release_key(key);
    }
}

#[cfg(target_os = "linux")]
fn clear_modifiers() {}

/// Synthesises a Ctrl+V keystroke into the foreground window.
#[cfg(windows)]
pub fn send_paste() -> Result<()> {
    const VK_V: u16 = 0x56;

    let inputs = [
        key_input(VK_CONTROL, false),
        key_input(VIRTUAL_KEY(VK_V), false),
        key_input(VIRTUAL_KEY(VK_V), true),
        key_input(VK_CONTROL, true),
    ];

    // SAFETY: `inputs` is a valid, correctly sized slice of INPUT structures.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err(AppError::msg(format!(
            "only {sent} of {} input events were accepted; \
             a window with higher integrity may be in the foreground",
            inputs.len()
        )));
    }
    Ok(())
}

/// Types `count` backspaces into the foreground window.
///
/// The `::` popup's trigger is not filtered out of the keystroke stream — both
/// characters land in the application being typed into — so it is taken back out
/// with the editing key. Virtual-key presses are enough here: unlike Ctrl+V this
/// has to work in the same window the user is already typing in.
#[cfg(windows)]
pub fn send_backspaces(count: usize) -> Result<()> {
    const VK_BACK: u16 = 0x08;

    let count = count.clamp(1, 8);
    let mut inputs = Vec::with_capacity(count * 2);
    for _ in 0..count {
        inputs.push(key_input(VIRTUAL_KEY(VK_BACK), false));
        inputs.push(key_input(VIRTUAL_KEY(VK_BACK), true));
    }

    // SAFETY: `inputs` is a valid, correctly sized slice of INPUT structures.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err(AppError::msg(format!(
            "only {sent} of {} input events were accepted; \
             a window with higher integrity may be in the foreground",
            inputs.len()
        )));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn send_backspaces(count: usize) -> Result<()> {
    if !crate::macos_permissions::accessibility_granted() {
        return Err(crate::macos_permissions::accessibility_denied());
    }    for _ in 0..count.clamp(1, 8) {
        cg::press(cg::KEY_DELETE);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn send_backspaces(count: usize) -> Result<()> {
    let count = count.clamp(1, 8).to_string();
    xdotool(&[
        "key",
        "--clearmodifiers",
        "--repeat",
        &count,
        "--delay",
        "20",
        "BackSpace",
    ])
    .map(|_| ())
}

/// Brings one of our own windows to the front, past the foreground lock.
///
/// The `::` popup opens while another application is in front, and Windows only
/// honours `SetForegroundWindow` for the process that owns the foreground. The
/// hook merely observed the keystroke that asked for the popup — it was never given
/// the input — so the window otherwise appears without the keyboard, which leaves
/// Escape and every click with the application behind it. Attaching to the
/// foreground thread's queue for the length of the call is the documented way
/// around that restriction.
#[cfg(windows)]
pub fn take_foreground(handle: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, IsWindow, SetForegroundWindow,
    };

    let window = HWND(handle as *mut core::ffi::c_void);
    // SAFETY: every call below only reads or adjusts windows that exist, and
    // IsWindow guards against one that has been closed in the meantime.
    unsafe {
        if !IsWindow(Some(window)).as_bool() {
            return false;
        }
        if SetForegroundWindow(window).as_bool() {
            return true;
        }
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() {
            return false;
        }
        let theirs = GetWindowThreadProcessId(foreground, None);
        let ours = GetCurrentThreadId();
        if theirs == 0 || theirs == ours {
            return false;
        }
        let attached = AttachThreadInput(ours, theirs, true).as_bool();
        let taken = SetForegroundWindow(window).as_bool();
        if attached {
            let _ = AttachThreadInput(ours, theirs, false);
        }
        taken
    }
}

#[cfg(target_os = "macos")]
pub fn take_foreground(handle: isize) -> bool {
    // macOS has no foreground lock to work around: activating the application puts
    // its window in front and gives it the keyboard.
    activate(handle)
}

#[cfg(target_os = "linux")]
pub fn take_foreground(handle: isize) -> bool {
    activate(handle)
}

/// The handful of CoreGraphics calls a synthetic keystroke needs.
///
/// Declared here rather than pulling in `objc2-core-graphics`: three functions,
/// two flags and the key codes is everything auto-paste uses, and the C ABI for
/// them has not moved since 10.4.
#[cfg(target_os = "macos")]
mod cg {
    use std::ffi::c_void;

    /// `kCGHIDEventTap`: the tap that sits closest to the keyboard.
    const HID_EVENT_TAP: u32 = 0;
    /// `kCGEventSourceStateHIDSystemState`.
    const HID_SYSTEM_STATE: u32 = 1;
    const FLAG_COMMAND: u64 = 0x0010_0000;

    /// `kVK_ANSI_V`.
    pub const KEY_V: u16 = 9;
    /// `kVK_Delete`, which is the Backspace key on a Mac keyboard.
    pub const KEY_DELETE: u16 = 51;
    pub const KEY_COMMAND: u16 = 55;
    pub const KEY_SHIFT: u16 = 56;
    pub const KEY_OPTION: u16 = 58;
    pub const KEY_CONTROL: u16 = 59;
    pub const KEY_RIGHT_SHIFT: u16 = 60;
    pub const KEY_RIGHT_OPTION: u16 = 61;
    pub const KEY_RIGHT_CONTROL: u16 = 62;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreateKeyboardEvent(
            source: *mut c_void,
            virtual_key: u16,
            key_down: bool,
        ) -> *mut c_void;
        fn CGEventSetFlags(event: *mut c_void, flags: u64);
        fn CGEventPost(tap: u32, event: *mut c_void);
        fn CGEventSourceCreate(state: u32) -> *mut c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(item: *const c_void);
    }

    /// Posts one key event and releases it again.
    fn post(source: *mut c_void, key: u16, down: bool, flags: u64) -> bool {
        // SAFETY: every object created here is released below, and the event pair
        // is posted once.
        unsafe {
            let event = CGEventCreateKeyboardEvent(source, key, down);
            if event.is_null() {
                return false;
            }
            CGEventSetFlags(event, flags);
            CGEventPost(HID_EVENT_TAP, event);
            CFRelease(event);
        }
        true
    }

    /// Presses and releases `key`, with no modifier held.
    pub fn press(key: u16) -> bool {
        keystroke(key, 0)
    }

    /// Presses and releases `key` with Command held, which is ⌘V and friends.
    pub fn command(key: u16) -> bool {
        keystroke(key, FLAG_COMMAND)
    }

    /// Releases `key` on its own, for dropping a modifier the user is still holding.
    pub fn release_key(key: u16) -> bool {
        // SAFETY: the source is released below.
        let source = unsafe { CGEventSourceCreate(HID_SYSTEM_STATE) };
        let posted = post(source, key, false, 0);
        // SAFETY: the source came from the call above; a null source is not a
        // resource and must not be released.
        unsafe {
            if !source.is_null() {
                CFRelease(source);
            }
        }
        posted
    }

    fn keystroke(key: u16, flags: u64) -> bool {
        // SAFETY: the source, and the two events it creates, are released below.
        unsafe {
            let source = CGEventSourceCreate(HID_SYSTEM_STATE);
            let posted = post(source, key, true, flags) && post(source, key, false, flags);
            if !source.is_null() {
                CFRelease(source);
            }
            posted
        }
    }
}

#[cfg(windows)]
fn key_input(key: VIRTUAL_KEY, key_up: bool) -> INPUT {    // Supplying the scan code as well as the virtual key keeps the synthetic input
    // working in applications that read hardware scan codes directly (games, some
    // terminals and remote-desktop clients), which ignore VK-only events.
    // SAFETY: MapVirtualKeyW is a pure lookup for a valid virtual key.
    let scan = unsafe { MapVirtualKeyW(key.0 as u32, MAPVK_VK_TO_VSC) } as u16;

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: scan,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(target_os = "macos")]
pub fn send_paste() -> Result<()> {
    // Posting into another application is exactly what the Accessibility grant is
    // for; without it macOS silently ignores the event. Asking here raises the
    // system dialog at the moment the user first wants a paste, which is where it
    // makes sense, and names the pane when the dialog has already been answered.
    if !crate::macos_permissions::request_accessibility() {
        return Err(crate::macos_permissions::accessibility_denied());
    }
    if !cg::command(cg::KEY_V) {
        return Err(AppError::msg("macOS refused the paste keystroke"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn send_paste() -> Result<()> {
    // `--clearmodifiers` takes the place of `clear_modifiers`: xdotool drops the
    // chord the user is still holding before it types Ctrl+V.
    log::debug!(
        "pasting into window {:?} (focus is now {:?})",
        target(),
        active_window()
    );
    xdotool(&["key", "--clearmodifiers", "ctrl+v"]).map(|_| ())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn key_input_marks_key_up_events() {
        // SAFETY: reading back the union fields we just initialised.
        let down = unsafe { key_input(VK_CONTROL, false).Anonymous.ki };
        assert_eq!(down.wVk, VK_CONTROL);
        assert_eq!(down.dwFlags, KEYBD_EVENT_FLAGS(0));
        assert!(
            down.wScan > 0,
            "scan code should be resolved for compatibility"
        );

        // SAFETY: as above.
        let up = unsafe { key_input(VK_CONTROL, true).Anonymous.ki };
        assert_eq!(up.dwFlags, KEYEVENTF_KEYUP);

        let letter = VIRTUAL_KEY(0x56);
        // SAFETY: as above.
        let v = unsafe { key_input(letter, false).Anonymous.ki };
        assert_eq!(v.wVk, letter);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "sends real keystrokes to the foreground window"]
    fn send_paste_reports_success() {
        assert!(send_paste().is_ok());
    }
}

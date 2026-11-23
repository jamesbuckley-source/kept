//! The macOS privacy grants Kept has to ask the user for.
//!
//! Two features sit behind a TCC grant: the screenshot tools and the colour
//! picker need Screen Recording, and auto-paste needs Accessibility. Neither can
//! be granted from code — macOS shows its own prompt, or the user flips a switch
//! in System Settings — so this module reports the state, asks once, and can open
//! the right pane for the user to finish the job.

use crate::error::{AppError, Result};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// Whether this process already holds the Screen Recording grant.
    fn CGPreflightScreenCaptureAccess() -> bool;
    /// Puts the system's Screen Recording prompt on screen. Answers whether the
    /// grant is held *now*, which is `false` while the user is still deciding.
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    /// Whether this process may post events into another application.
    fn AXIsProcessTrusted() -> bool;
}

/// True when the app may read the screen.
pub fn screen_recording_granted() -> bool {
    // SAFETY: a C function with no arguments that only reads the current grant.
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Asks for Screen Recording, showing the system prompt when the grant is missing.
///
/// The prompt is asynchronous, so this answers `false` on the first ask: the
/// capture then fails with [`screen_recording_denied`], which says where the
/// switch is. A second attempt after the user allows it works without a restart
/// on every macOS this app targets.
pub fn request_screen_recording() -> bool {
    if screen_recording_granted() {
        return true;
    }
    // SAFETY: as above; this is the documented way to ask.
    unsafe { CGRequestScreenCaptureAccess() }
}

/// True when the app may post keystrokes into other applications.
pub fn accessibility_granted() -> bool {
    // SAFETY: a C function with no arguments that only reads the current grant.
    unsafe { AXIsProcessTrusted() }
}

/// The error a capture raises when macOS has not been given the screen.
pub fn screen_recording_denied() -> AppError {
    AppError::msg(
        "macOS has not let Kept record the screen yet. Allow it in System Settings → \
         Privacy & Security → Screen & Audio Recording, then try again.",
    )
}

/// The error auto-paste raises when macOS has not been given the keyboard.
pub fn accessibility_denied() -> AppError {
    AppError::msg(
        "macOS has not let Kept press keys for you yet. Allow it in System Settings → \
         Privacy & Security → Accessibility, then try again.",
    )
}

/// Which System Settings pane a grant lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    ScreenRecording,
    Accessibility,
}

impl Pane {
    /// The query string the `x-apple.systempreferences:` URL takes for this pane.
    fn anchor(self) -> &'static str {
        match self {
            Pane::ScreenRecording => "Privacy_ScreenCapture",
            Pane::Accessibility => "Privacy_Accessibility",
        }
    }
}

/// Opens the System Settings pane that holds one grant.
///
/// A grant that macOS has already refused is only changeable there, and a
/// deep link is the difference between "allow it in Settings" and the user
/// hunting through a list of twenty panes.
pub fn open_settings(pane: Pane) -> Result<()> {
    let target =
        format!("x-apple.systempreferences:com.apple.preference.security?{}", pane.anchor());
    let status = std::process::Command::new("/usr/bin/open")
        .arg(&target)
        .status()
        .map_err(|e| AppError::msg(format!("could not open System Settings: {e}")))?;
    if !status.success() {
        return Err(AppError::msg("System Settings could not be opened"));
    }
    Ok(())
}

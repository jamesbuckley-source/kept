//! Screen capture on macOS, through the system's own screenshot tool.
//!
//! `screencapture` is the only way to get the native selector: it dims the
//! screen, draws the crosshair, accepts Space to switch from a region to a window
//! and Escape to cancel. macOS attributes the Screen Recording grant to Kept,
//! because Kept is the process that launched it.
//!
//! The result is a plain [`ClipContent::Image`], so the store's ordinary path
//! supplies thumbnails, de-duplication, OCR and the history event.
//!
//! The pure helpers here — locating a selected piece inside a picture of the
//! desktop, and turning its position into screen points — are kept apart from the
//! shell-out so they can be tested like the picker's colour helpers.

use std::path::Path;
use std::sync::Mutex;

use crate::capture::Rect;
use crate::error::{AppError, Result};
use crate::macos_permissions;
use crate::models::ClipContent;

/// The system screenshot tool, by full path: the `PATH` a launchd-started app
/// inherits is not the one a login shell has.
const SCREENCAPTURE: &str = "/usr/sbin/screencapture";

/// Pixels sampled along a row before the whole row is compared.
const SAMPLES: usize = 8;

/// Where the last capture was taken, so "previous area" can repeat it.
///
/// In points, in the global screen space, because that is what `screencapture -R`
/// takes. Kept for the session only, exactly as on Windows.
static LAST_AREA: Mutex<Option<Rect>> = Mutex::new(None);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetActiveDisplayList(
        max_displays: u32,
        displays: *mut u32,
        display_count: *mut u32,
    ) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
}

/// Enough for every realistic arrangement of displays.
const MAX_DISPLAYS: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

/// Every active display as one rectangle, in points, with the top-left of the main
/// display as the origin.
///
/// CoreGraphics reports display bounds in that space already, with secondary
/// displays to the left or above the main one carrying negative coordinates.
pub fn desktop_bounds() -> Rect {
    let mut displays = [0u32; MAX_DISPLAYS];
    let mut count = 0u32;
    // SAFETY: the buffer and the count are locals that outlive the call.
    let status = unsafe {
        CGGetActiveDisplayList(MAX_DISPLAYS as u32, displays.as_mut_ptr(), &mut count)
    };
    if status != 0 || count == 0 {
        return Rect::new(0, 0, 0, 0);
    }
    let mut union: Option<Rect> = None;
    for display in &displays[..count as usize] {
        // SAFETY: an id that came from the list above.
        let frame = unsafe { CGDisplayBounds(*display) };
        let rect = Rect::new(
            frame.origin.x.round() as i32,
            frame.origin.y.round() as i32,
            (frame.origin.x + frame.size.width).round() as i32,
            (frame.origin.y + frame.size.height).round() as i32,
        );
        if rect.is_empty() {
            continue;
        }
        union = Some(match union {
            None => rect,
            Some(acc) => Rect::new(
                acc.left.min(rect.left),
                acc.top.min(rect.top),
                acc.right.max(rect.right),
                acc.bottom.max(rect.bottom),
            ),
        });
    }
    union.unwrap_or(Rect::new(0, 0, 0, 0))
}

/// The area the last capture covered, if it was worked out.
pub fn last_area() -> Option<Rect> {
    *LAST_AREA.lock().unwrap_or_else(|e| e.into_inner())
}

/// Lets the user frame a rectangle. Escape is a cancel.
pub fn region() -> Result<Option<ClipContent>> {
    interactive(&["-i"])
}

/// Lets the user click a window. `-o` leaves the window's shadow out, so the
/// picture is the window itself rather than the window floating over a desktop.
pub fn window() -> Result<Option<ClipContent>> {
    interactive(&["-w", "-o"])
}

/// Every display as one picture, the way the system's own fullscreen shot does.
pub fn screen() -> Result<ClipContent> {
    let shot = grab(&[])?.ok_or_else(|| AppError::msg("screencapture did not produce a picture"))?;
    let bounds = desktop_bounds();
    if !bounds.is_empty() {
        *LAST_AREA.lock().unwrap_or_else(|e| e.into_inner()) = Some(bounds);
    }
    Ok(shot)
}

/// Repeats a capture of the same rectangle.
pub fn area(rect: Rect) -> Result<ClipContent> {
    if rect.is_empty() {
        return Err(AppError::msg("there is no area to capture"));
    }
    let spec = format!(
        "{},{},{},{}",
        rect.left,
        rect.top,
        rect.width(),
        rect.height()
    );
    grab(&["-R", &spec])?.ok_or_else(|| AppError::msg("screencapture did not produce a picture"))
}

/// Runs one interactive capture, and works out the rectangle it covered.
///
/// macOS freezes the screen while its selector is up, so the picture that comes
/// back is a piece of a picture taken an instant earlier. Finding that piece is
/// how the rectangle of a selection is known without a selector of our own —
/// which is what "capture the previous area" needs, and there is no other way to
/// ask `screencapture` what was chosen.
fn interactive(selector: &[&str]) -> Result<Option<ClipContent>> {
    let before = grab(&[]).ok().flatten().and_then(image_of);
    let Some(shot) = grab(selector)? else {
        return Ok(None);
    };
    match (&before, image_of(shot.clone())) {
        (Some((width, height, rgba)), Some((part_width, part_height, part))) => {
            match locate(
                rgba,
                (*width, *height),
                &part,
                (part_width, part_height),
            ) {
                Some((x, y)) => {
                    if let Some(area) = pixel_area(
                        desktop_bounds(),
                        (*width, *height),
                        (x, y),
                        (part_width, part_height),
                    ) {
                        *LAST_AREA.lock().unwrap_or_else(|e| e.into_inner()) = Some(area);
                    }
                }
                None => {
                    log::info!("could not work out where the selection came from; it will not be repeatable")
                }
            }
        }
        _ => log::info!("the selection was taken without a picture of the desktop to compare"),
    }
    Ok(Some(shot))
}

fn image_of(content: ClipContent) -> Option<(u32, u32, Vec<u8>)> {
    match content {
        ClipContent::Image {
            width,
            height,
            rgba,
        } => Some((width, height, rgba)),
        _ => None,
    }
}

/// Runs `screencapture` with `args` and decodes the PNG it writes.
///
/// `Ok(None)` means the user cancelled: Escape in the selector leaves no file
/// behind. A refusal is reported as an error, because that is something the user
/// has to go and fix.
fn grab(args: &[&str]) -> Result<Option<ClipContent>> {
    if !macos_permissions::screen_recording_granted()
        && !macos_permissions::request_screen_recording()
    {
        log::warn!("the screen capture was refused: Screen Recording is not granted");
        return Err(macos_permissions::screen_recording_denied());
    }
    let path = std::env::temp_dir().join(format!("kept-shot-{}.png", uuid::Uuid::new_v4()));
    let output = std::process::Command::new(SCREENCAPTURE)
        .args(args)
        // No shutter sound: a capture started from a menu should be silent.
        .arg("-x")
        .arg(&path)
        .output()
        .map_err(|e| AppError::msg(format!("could not run {SCREENCAPTURE}: {e}")))?;
    let wrote = path
        .metadata()
        .map(|meta| meta.len() > 0)
        .unwrap_or(false);
    if !wrote {
        let _ = std::fs::remove_file(&path);
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if detail.contains("could not create image") {
            return Err(macos_permissions::screen_recording_denied());
        }
        if output.status.success() {
            return Err(AppError::msg(format!(
                "screencapture saved nothing{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            )));
        }
        log::info!("the screen capture was cancelled");
        return Ok(None);
    }
    decode(&path).map(Some)
}

fn decode(path: &Path) -> Result<ClipContent> {
    let frame = image::open(path).map(|frame| frame.to_rgba8()).map_err(|e| {
        AppError::msg(format!(
            "could not read the capture from {}: {e}",
            path.display()
        ))
    });
    let _ = std::fs::remove_file(path);
    let frame = frame?;
    Ok(ClipContent::Image {
        width: frame.width(),
        height: frame.height(),
        rgba: frame.into_raw(),
    })
}

/// Finds where `part` sits inside `desktop`, in pixels.
///
/// A row through the middle of the piece is searched for first; the rows above and
/// below confirm a hit, so a stretch of wall-to-wall toolbar the same colour as
/// the piece cannot pass for it on its own.
pub fn locate(
    desktop: &[u8],
    desktop_size: (u32, u32),
    part: &[u8],
    part_size: (u32, u32),
) -> Option<(u32, u32)> {
    let (desktop_width, desktop_height) = desktop_size;
    let (part_width, part_height) = part_size;
    if part_width == 0
        || part_height == 0
        || part_width > desktop_width
        || part_height > desktop_height
    {
        return None;
    }
    if desktop.len() < desktop_width as usize * desktop_height as usize * 4
        || part.len() < part_width as usize * part_height as usize * 4
    {
        return None;
    }

    let row_bytes = part_width as usize * 4;
    let row = |index: u32| -> &[u8] { &part[index as usize * row_bytes..][..row_bytes] };
    let band = part_height / 2;
    let confirmations = [part_height / 4, (part_height * 3) / 4];

    for y in 0..=(desktop_height - part_height) {
        for x in 0..=(desktop_width - part_width) {
            if !row_matches(desktop, desktop_width, x, y + band, row(band)) {
                continue;
            }
            let agree = confirmations
                .iter()
                .filter(|row_index| {
                    row_matches(desktop, desktop_width, x, y + *row_index, row(**row_index))
                })
                .count();
            // One confirmation is enough: a window's title bar changes colour when
            // it is clicked, so the row nearest it may no longer match.
            if agree > 0 {
                return Some((x, y));
            }
        }
    }
    None
}

/// Whether the desktop row at `y`, starting at `x`, is `row` pixel for pixel.
fn row_matches(desktop: &[u8], width: u32, x: u32, y: u32, row: &[u8]) -> bool {
    let start = (y as usize * width as usize + x as usize) * 4;
    let Some(candidate) = desktop.get(start..start + row.len()) else {
        return false;
    };
    // A few pixels spread along the row reject almost every position before the
    // whole row is compared, which is what keeps a full-screen search quick.
    let pixels = row.len() / 4;
    for sample in 0..SAMPLES {
        let pixel = (pixels - 1) * sample / (SAMPLES - 1);
        let at = pixel * 4;
        if candidate[at..at + 4] != row[at..at + 4] {
            return false;
        }
    }
    candidate == row
}

/// Turns a position in the picture of the desktop into the screen rectangle it
/// covers, in points.
///
/// The picture is at the displays' pixel size while `-R` takes points, so the
/// ratio of the two is the scale. `bounds` is the point-space rectangle the
/// picture covers, which is the whole desktop including any display to the left
/// of or above the main one.
pub fn pixel_area(
    bounds: Rect,
    image: (u32, u32),
    at: (u32, u32),
    size: (u32, u32),
) -> Option<Rect> {
    let (image_width, image_height) = image;
    if bounds.is_empty() || image_width == 0 || image_height == 0 {
        return None;
    }
    let scale = f64::from(image_width) / f64::from(bounds.width());
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let points = |value: u32| (f64::from(value) / scale).round() as i32;
    let left = bounds.left + points(at.0);
    let top = bounds.top + points(at.1);
    Some(Rect::new(
        left,
        top,
        left + points(size.0).max(1),
        top + points(size.1).max(1),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small "screen" whose every pixel is distinct, so a piece of it can only
    /// match where it really came from.
    fn desktop(width: u32, height: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[x as u8, y as u8, (x * 7 + y * 13) as u8, 255]);
            }
        }
        rgba
    }

    fn piece(desktop: &[u8], width: u32, x: u32, y: u32, piece_width: u32, piece_height: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(piece_width as usize * piece_height as usize * 4);
        for row in 0..piece_height {
            let start = ((y + row) as usize * width as usize + x as usize) * 4;
            out.extend_from_slice(&desktop[start..start + piece_width as usize * 4]);
        }
        out
    }

    #[test]
    fn a_selection_is_located_in_the_desktop_it_came_from() {
        let frame = desktop(40, 30);
        let part = piece(&frame, 40, 12, 7, 9, 6);
        assert_eq!(locate(&frame, (40, 30), &part, (9, 6)), Some((12, 7)));
    }

    #[test]
    fn a_selection_that_is_not_there_is_not_located() {
        let frame = desktop(40, 30);
        let mut part = piece(&frame, 40, 5, 5, 4, 4);
        // Change the middle row, which is the one the search runs on.
        let row_bytes = 4 * 4;
        for byte in &mut part[2 * row_bytes..3 * row_bytes] {
            *byte = byte.wrapping_add(1);
        }
        assert_eq!(locate(&frame, (40, 30), &part, (4, 4)), None);
    }

    #[test]
    fn a_piece_larger_than_the_desktop_is_refused() {
        let frame = desktop(10, 10);
        assert_eq!(locate(&frame, (10, 10), &frame, (11, 10)), None);
        assert_eq!(locate(&frame, (10, 10), &frame, (10, 0)), None);
    }

    #[test]
    fn a_retina_selection_becomes_the_points_it_covers() {
        // A 1920x1080 desktop photographed at 3840x2160, with a 200x300 piece at
        // (100, 50) in the picture: a quarter of that in points.
        let bounds = Rect::new(0, 0, 1920, 1080);
        assert_eq!(
            pixel_area(bounds, (3840, 2160), (100, 50), (200, 300)),
            Some(Rect::new(50, 25, 150, 175))
        );
    }

    #[test]
    fn a_desktop_that_starts_left_of_the_main_display_keeps_its_origin() {
        // A second display to the left of the main one moves the whole union
        // negative, and a selection has to come back in those same coordinates.
        let bounds = Rect::new(-1440, 0, 1920, 1080);
        assert_eq!(
            pixel_area(bounds, (3360, 1080), (0, 0), (100, 100)),
            Some(Rect::new(-1440, 0, -1340, 100))
        );
    }
}

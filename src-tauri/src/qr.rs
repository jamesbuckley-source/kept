//! Finding the QR code in a picture.
//!
//! The decoder is pure Rust (`rqrr`), so a code copied on one platform reads the
//! same on the others. It is used in two places: the clipboard watcher reads any
//! picture it stores, and the "Capture QR code" tool reads a region of the screen.

use rqrr::PreparedImage;

/// Reads every QR code in a row-major RGBA picture.
///
/// A screenshot can hold more than one code, so the caller gets them all; the
/// payloads come back in the order the detector found them.
pub fn decode_rgba(rgba: &[u8], width: usize, height: usize) -> Vec<String> {
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return Vec::new();
    }
    // Rec. 601 luma: the decoder only needs contrast, and averaging the channels
    // would read a saturated blue code as mid grey.
    let mut luma = Vec::with_capacity(width * height);
    for pixel in rgba.chunks_exact(4) {
        let y = 0.299 * pixel[0] as f32 + 0.587 * pixel[1] as f32 + 0.114 * pixel[2] as f32;
        luma.push(y.round().clamp(0.0, 255.0) as u8);
    }
    decode_luma(&luma, width, height)
}

/// Reads every QR code in a PNG or JPEG picture, as stored for a clip.
///
/// Big pictures are shrunk first: detection cost grows with the pixel count and a
/// code stays readable well below the size a screenshot arrives at.
pub fn decode_encoded(bytes: &[u8]) -> Vec<String> {
    const MAX_EDGE: u32 = 2048;
    let Ok(image) = image::load_from_memory(bytes) else {
        return Vec::new();
    };
    let image = if image.width() > MAX_EDGE || image.height() > MAX_EDGE {
        image.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    decode_rgba(rgba.as_raw(), width as usize, height as usize)
}

fn decode_luma(luma: &[u8], width: usize, height: usize) -> Vec<String> {
    let mut prepared = PreparedImage::prepare_from_greyscale(width, height, |x, y| {
        luma[y as usize * width + x as usize]
    });
    let mut found = Vec::new();
    for grid in prepared.detect_grids() {
        if let Ok((_meta, payload)) = grid.decode() {
            let payload = payload.trim().to_string();
            if !payload.is_empty() && !found.contains(&payload) {
                found.push(payload);
            }
        }
    }
    found
}

/// Turns a scanned payload into something a browser can open.
///
/// Scanned codes are usually links, but plenty are printed as `example.com/x`
/// with no scheme, which is not a URL yet.
pub fn web_target(payload: &str) -> Option<String> {
    let text = payload.trim();
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(text.to_string());
    }
    // Anything with whitespace or no dot at all is text, not an address.
    if text.is_empty() || text.contains(char::is_whitespace) {
        return None;
    }
    let host = text.split(['/', '?', '#']).next().unwrap_or_default();
    let host_name = host.rsplit('@').next().unwrap_or_default();
    let host_name = host_name.split(':').next().unwrap_or_default();
    let is_domain = host_name.contains('.')
        && !host_name.starts_with('.')
        && !host_name.ends_with('.')
        && host_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
    is_domain.then(|| format!("https://{text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes `payload` as a QR code and renders it to RGBA, the way a
    /// screenshot of a real code would arrive.
    fn qr_picture(payload: &str) -> (Vec<u8>, usize, usize) {
        use qrcode::QrCode;
        let code = QrCode::new(payload.as_bytes()).expect("encode");
        let modules = code.to_colors();
        let side = code.width();
        let scale = 8;
        let quiet = 4 * scale;
        let size = side * scale + quiet * 2;
        let mut rgba = vec![255u8; size * size * 4];
        for y in 0..size {
            for x in 0..size {
                let dark = x >= quiet
                    && y >= quiet
                    && {
                        let mx = (x - quiet) / scale;
                        let my = (y - quiet) / scale;
                        mx < side && my < side && modules[my * side + mx] == qrcode::Color::Dark
                    };
                if dark {
                    let at = (y * size + x) * 4;
                    rgba[at] = 0;
                    rgba[at + 1] = 0;
                    rgba[at + 2] = 0;
                }
            }
        }
        (rgba, size, size)
    }

    #[test]
    fn reads_a_link_out_of_a_picture() {
        let (rgba, w, h) = qr_picture("https://example.com/order/42");
        assert_eq!(
            decode_rgba(&rgba, w, h),
            vec!["https://example.com/order/42".to_string()]
        );
    }

    #[test]
    fn reads_plain_text_codes_too() {
        let (rgba, w, h) = qr_picture("WIFI:T:WPA;S:Cafe;P:espresso;;");
        assert_eq!(
            decode_rgba(&rgba, w, h),
            vec!["WIFI:T:WPA;S:Cafe;P:espresso;;".to_string()]
        );
    }

    #[test]
    fn reads_a_code_out_of_an_encoded_picture() {
        let (rgba, w, h) = qr_picture("https://kept.app/order/42");
        let picture = image::RgbaImage::from_raw(w as u32, h as u32, rgba).expect("raw pixels");
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(picture)
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("encode png");
        assert_eq!(
            decode_encoded(png.get_ref()),
            vec!["https://kept.app/order/42".to_string()]
        );
        // Bytes that are not a picture at all read as nothing rather than failing.
        assert!(decode_encoded(b"not a picture").is_empty());
    }

    #[test]
    fn a_picture_without_a_code_reads_as_nothing() {
        let rgba = vec![200u8; 64 * 64 * 4];
        assert!(decode_rgba(&rgba, 64, 64).is_empty());
        assert!(decode_rgba(&[], 0, 0).is_empty());
        assert!(decode_rgba(&[0, 0, 0], 64, 64).is_empty());
    }

    #[test]
    fn web_targets_keep_links_and_adopt_bare_hosts() {
        assert_eq!(
            web_target("https://kept.app/download").as_deref(),
            Some("https://kept.app/download")
        );
        assert_eq!(
            web_target("HTTP://EXAMPLE.COM/A").as_deref(),
            Some("HTTP://EXAMPLE.COM/A")
        );
        assert_eq!(
            web_target("example.com/order?id=1").as_deref(),
            Some("https://example.com/order?id=1")
        );
        assert_eq!(
            web_target("user@example.com").as_deref(),
            Some("https://user@example.com")
        );
    }

    #[test]
    fn web_targets_reject_things_that_are_not_addresses() {
        assert_eq!(web_target(""), None);
        assert_eq!(web_target("   "), None);
        assert_eq!(web_target("hello world"), None);
        assert_eq!(web_target("meeting at 10:30"), None);
        assert_eq!(web_target("12345"), None);
        assert_eq!(web_target(".com"), None);
        assert_eq!(web_target("example.c om"), None);
    }
}

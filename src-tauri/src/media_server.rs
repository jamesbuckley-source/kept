//! A loopback HTTP server for the media the webview has to play.
//!
//! WebKitGTK hands a `<video>` to GStreamer, and GStreamer only opens addresses its
//! own sources understand: http(s), file, and whatever the page can hand it as a
//! blob. A `media://` URL is none of those, so a recording failed with a format
//! error — "src not supported" — while the same bytes played from `http://localhost`
//! in the same webview. Images are unaffected: those are fetched by the network
//! stack, which has no such limit, so only video had to move.
//!
//! The port is picked by the kernel and the first path segment is a secret drawn
//! per run, so nothing else on the machine can read the history through it without
//! knowing both. The handler is the same [`crate::serve_media`] the `media:` scheme
//! uses, so the allowlist, sealing, caching, content type and range handling all
//! stay in one place.

use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;

/// Longest request line or header block that will be read from one connection.
const MAX_REQUEST_BYTES: usize = 8 * 1024;

/// The prefix every video URL begins with, once the server is running.
static BASE: OnceLock<String> = OnceLock::new();

/// Starts the server and answers with the URL prefix it serves from.
///
/// `None` means the port could not be taken or the thread could not be started.
/// The app still works; videos just keep the address that cannot play them.
pub fn start(app: tauri::AppHandle) -> Option<String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let base = format!("http://127.0.0.1:{port}/{token}");
    let serving = token.clone();
    let spawned = std::thread::Builder::new()
        .name("media-server".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let app = app.clone();
                let token = serving.clone();
                // A player opens a handful of connections at once, and one that
                // stalls mid-seek must not hold up the next.
                let _ = std::thread::Builder::new()
                    .name("media-request".into())
                    .spawn(move || serve(&app, stream, &token));
            }
        });
    match spawned {
        Ok(_) => {
            let _ = BASE.set(base.clone());
            Some(base)
        }
        Err(e) => {
            log::warn!("could not start the media server: {e}");
            None
        }
    }
}

/// The prefix videos are served from, or `None` before [`start`] has succeeded.
pub fn base() -> Option<&'static str> {
    BASE.get().map(String::as_str)
}

/// Answers one connection: the request line, the headers it cares about, the file.
fn serve(app: &tauri::AppHandle, stream: TcpStream, token: &str) {
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let mut reader = std::io::BufReader::new(stream);
    let Some(request) = read_request(&mut reader) else {
        let _ = write_status(&mut out, 400, "Bad Request");
        return;
    };
    if request.method != "GET" && request.method != "HEAD" {
        let _ = write_status(&mut out, 405, "Method Not Allowed");
        return;
    }
    let Some(name) = requested_name(&request.target, token) else {
        let _ = write_status(&mut out, 404, "Not Found");
        return;
    };
    let response = crate::serve_media(app, &name, request.range.as_deref());
    let _ = write_response(&mut out, response, request.method == "HEAD");
}

/// What a request asked for, once its headers have been read.
struct Request {
    method: String,
    target: String,
    /// The `Range` header as sent, which `serve_media` already knows how to answer.
    range: Option<String>,
}

/// Reads the request line and headers, and nothing else.
fn read_request(reader: &mut impl BufRead) -> Option<Request> {
    let mut spent = 0usize;
    let line = read_line(reader, &mut spent)?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_ascii_uppercase();
    let target = parts.next()?.to_string();
    let mut range = None;
    loop {
        let header = read_line(reader, &mut spent)?;
        let header = header.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("range") {
            range = Some(value.trim().to_string());
        }
    }
    Some(Request {
        method,
        target,
        range,
    })
}

/// One line of the request, refusing anything that would run away with memory.
fn read_line(reader: &mut impl BufRead, spent: &mut usize) -> Option<String> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                bytes.push(byte[0]);
                if byte[0] == b'\n' || bytes.len() >= MAX_REQUEST_BYTES {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    if bytes.is_empty() {
        return None;
    }
    *spent += bytes.len();
    if *spent > MAX_REQUEST_BYTES {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The file a request path names, provided it starts with this run's secret.
///
/// `/secret/9.mp4` answers `9.mp4`; any other first segment answers nothing, which
/// is what keeps the history unreadable to anything else on the machine.
fn requested_name(target: &str, token: &str) -> Option<String> {
    let rest = target.strip_prefix('/')?;
    let (given, name) = rest.split_once('/')?;
    if given != token || name.is_empty() {
        return None;
    }
    let name = crate::clipboard::percent_decode(name);
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

/// Writes the headers the scheme handler built, then the body it produced.
fn write_response(
    stream: &mut TcpStream,
    response: tauri::http::Response<Vec<u8>>,
    head_only: bool,
) -> std::io::Result<()> {
    let (parts, body) = response.into_parts();
    let status = parts.status.as_u16();
    let reason = parts.status.canonical_reason().unwrap_or("");
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in parts.headers.iter() {
        // The length is written from the body below, so a stray header cannot make
        // the two disagree.
        if name.as_str().eq_ignore_ascii_case("content-length") {
            continue;
        }
        if let Ok(value) = value.to_str() {
            head.push_str(&format!("{}: {value}\r\n", name.as_str()));
        }
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    stream.write_all(head.as_bytes())?;
    if !head_only {
        stream.write_all(&body)?;
    }
    stream.flush()
}

/// A bare status line and an empty body, for requests that are not for a file.
fn write_status(stream: &mut TcpStream, status: u16, reason: &str) -> std::io::Result<()> {
    let head = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_token_is_the_first_segment_of_the_path() {
        assert_eq!(requested_name("/abc/9.mp4", "abc").as_deref(), Some("9.mp4"));
        assert_eq!(
            requested_name("/abc/custom-12ab.gif", "abc").as_deref(),
            Some("custom-12ab.gif")
        );
        // A guessed path, or one naming nothing, is refused.
        assert_eq!(requested_name("/def/9.mp4", "abc"), None);
        assert_eq!(requested_name("/abc/", "abc"), None);
        assert_eq!(requested_name("/abc", "abc"), None);
        assert_eq!(requested_name("/abc/../secret.png", "abc"), None);
        assert_eq!(requested_name("9.mp4", "abc"), None);
    }

    #[test]
    fn a_percent_escaped_name_is_decoded() {
        assert_eq!(
            requested_name("/abc/a%20b.mp4", "abc").as_deref(),
            Some("a b.mp4")
        );
    }

    #[test]
    fn a_request_line_and_its_range_are_read() {
        let raw = b"GET /tok/9.mp4 HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=100-\r\n\r\n";
        let request = read_request(&mut std::io::Cursor::new(&raw[..])).expect("a request");
        assert_eq!(request.method, "GET");
        assert_eq!(request.target, "/tok/9.mp4");
        assert_eq!(request.range.as_deref(), Some("bytes=100-"));
    }

    #[test]
    fn a_request_without_a_range_has_none() {
        let raw = b"GET /tok/9.mp4 HTTP/1.1\r\n\r\n";
        let request = read_request(&mut std::io::Cursor::new(&raw[..])).expect("a request");
        assert_eq!(request.range, None);
    }

    #[test]
    fn an_endless_header_block_is_refused() {
        let mut raw = b"GET /tok/9.mp4 HTTP/1.1\r\n".to_vec();
        for _ in 0..400 {
            raw.extend_from_slice(b"X-Filler: 0123456789012345678901234567890123\r\n");
        }
        assert!(read_request(&mut std::io::Cursor::new(raw)).is_none());
    }
}

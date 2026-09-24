//! Minimal HTTP/1.1 wire codec for the discovery directory API.
//!
//! Deliberately small: one request per connection (`Connection:
//! close`), `Content-Length` bodies only, hard caps on head and body
//! size. The directory is internal control-plane infrastructure — both
//! ends are this crate — so a restricted subset is the correct
//! security posture. Every parser path is fuzz-covered.

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::DiscoveryError;

/// Maximum head size (request/status line + headers) in bytes.
pub const MAX_HEAD: usize = 8 * 1024;
/// Maximum body size in bytes. Records are a few hundred bytes; the
/// registry snapshot is the largest object and stays under 64 KiB.
pub const MAX_BODY: usize = 256 * 1024;

/// A parsed request: method, path (no query support — routes don't
/// need it), and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

/// A response to write or a parsed response on the client side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, body: impl serde::Serialize) -> Self {
        Self {
            status,
            body: serde_json::to_vec(&body).unwrap_or_else(|_| b"null".to_vec()),
        }
    }

    pub fn text(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into().into_bytes(),
        }
    }

    pub fn error(status: u16, err: &DiscoveryError) -> Self {
        Self::json(status, serde_json::json!({ "error": err.to_string() }))
    }
}

fn bad(msg: &str) -> DiscoveryError {
    DiscoveryError::InvalidRecord(format!("http: {msg}"))
}

/// Read one request: head capped at [`MAX_HEAD`], body at [`MAX_BODY`].
/// Returns `Ok(None)` on a clean idle close (client connected and sent
/// nothing), `Err` on any malformed input.
pub async fn read_request(
    stream: &mut (impl AsyncReadExt + Unpin),
) -> Result<Option<Request>, DiscoveryError> {
    let head = match read_head(stream).await? {
        Some(h) => h,
        None => return Ok(None),
    };
    let head_str = std::str::from_utf8(&head).map_err(|_| bad("head is not utf-8"))?;
    let mut lines = head_str.split("\r\n");
    let request_line = lines.next().ok_or_else(|| bad("empty head"))?;
    let mut parts = request_line.split(' ');
    let method = parts.next().ok_or_else(|| bad("no method"))?.to_string();
    let path = parts.next().ok_or_else(|| bad("no path"))?.to_string();
    let version = parts.next().ok_or_else(|| bad("no version"))?;
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Err(bad("malformed request line"));
    }
    if !method.bytes().all(|b| b.is_ascii_alphabetic()) || method.len() > 16 {
        return Err(bad("bad method"));
    }
    if !path.starts_with('/') || path.len() > 512 {
        return Err(bad("bad path"));
    }
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or_else(|| bad("bad header"))?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            content_length = value
                .trim()
                .parse::<usize>()
                .map_err(|_| bad("bad content-length"))?;
        } else if name.trim().eq_ignore_ascii_case("transfer-encoding") {
            return Err(bad("transfer-encoding unsupported"));
        }
    }
    if content_length > MAX_BODY {
        return Err(bad("body too large"));
    }
    let mut body = vec![0u8; content_length];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|_| bad("body truncated"))?;
    Ok(Some(Request { method, path, body }))
}

/// Read a response head + body (client side). Same caps as requests.
pub async fn read_response(
    stream: &mut (impl AsyncReadExt + Unpin),
) -> Result<Response, DiscoveryError> {
    let head = read_head(stream)
        .await?
        .ok_or_else(|| bad("empty response"))?;
    let head_str = std::str::from_utf8(&head).map_err(|_| bad("head is not utf-8"))?;
    let mut lines = head_str.split("\r\n");
    let status_line = lines.next().ok_or_else(|| bad("empty head"))?;
    let mut parts = status_line.splitn(3, ' ');
    if !parts.next().is_some_and(|v| v.starts_with("HTTP/1.")) {
        return Err(bad("malformed status line"));
    }
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad("bad status code"))?;
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or_else(|| bad("bad header"))?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            content_length = value
                .trim()
                .parse::<usize>()
                .map_err(|_| bad("bad content-length"))?;
        }
    }
    if content_length > MAX_BODY {
        return Err(bad("body too large"));
    }
    let mut body = vec![0u8; content_length];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|_| bad("body truncated"))?;
    Ok(Response { status, body })
}

/// Serialize a request onto the stream.
pub async fn write_request(
    stream: &mut (impl AsyncWriteExt + Unpin),
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<(), DiscoveryError> {
    write_request_with_host(stream, "localhost", method, path, body).await
}

/// Serialize a request with the configured origin authority (also used by
/// HTTP/1.1 virtual hosts). Reject header injection before any bytes are sent.
pub async fn write_request_with_host(
    stream: &mut (impl AsyncWriteExt + Unpin),
    authority: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<(), DiscoveryError> {
    if authority.is_empty()
        || authority.len() > 2048
        || !authority
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    {
        return Err(bad("invalid host authority"));
    }
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|e| DiscoveryError::Store(e.to_string()))?;
    stream
        .write_all(body)
        .await
        .map_err(|e| DiscoveryError::Store(e.to_string()))?;
    stream
        .flush()
        .await
        .map_err(|e| DiscoveryError::Store(e.to_string()))
}

/// Serialize a response onto the stream.
pub async fn write_response(
    stream: &mut (impl AsyncWriteExt + Unpin),
    resp: &Response,
) -> Result<(), DiscoveryError> {
    let reason = match resp.status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
        resp.status,
        reason,
        resp.body.len()
    );
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|e| DiscoveryError::Store(e.to_string()))?;
    stream
        .write_all(&resp.body)
        .await
        .map_err(|e| DiscoveryError::Store(e.to_string()))?;
    stream
        .flush()
        .await
        .map_err(|e| DiscoveryError::Store(e.to_string()))
}

/// Read until `\r\n\r\n` with a hard cap; `Ok(None)` on clean EOF with
/// zero bytes (keep-alive style idle close).
async fn read_head(
    stream: &mut (impl AsyncReadExt + Unpin),
) -> Result<Option<Vec<u8>>, DiscoveryError> {
    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|e| DiscoveryError::Store(e.to_string()))?;
        if n == 0 {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Err(bad("head truncated"))
            };
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            return Ok(Some(buf));
        }
        if buf.len() > MAX_HEAD {
            return Err(bad("head too large"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest::proptest! {
        /// Arbitrary bytes through both parsers must never panic —
        /// every malformed input is a clean `Err`, including inputs
        /// that straddle the MAX_HEAD boundary.
        #[test]
        fn parsers_never_panic(
            bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..=9000),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            let _ = rt.block_on(read_request(&mut std::io::Cursor::new(bytes.clone())));
            let _ = rt.block_on(read_response(&mut std::io::Cursor::new(bytes)));
        }

        /// A well-formed request round-trips through the wire codec.
        #[test]
        fn request_roundtrip(
            method in "[A-Z]{3,8}",
            path in "/[a-z0-9/]{1,64}",
            body in proptest::collection::vec(proptest::num::u8::ANY, 0..=1024),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            let mut buf = Vec::new();
            rt.block_on(async {
                write_request(&mut buf, &method, &path, &body).await.unwrap();
                let mut cur = std::io::Cursor::new(buf.clone());
                let req = read_request(&mut cur).await.unwrap().unwrap();
                proptest::prop_assert_eq!(req.method, method.clone());
                proptest::prop_assert_eq!(req.path, path.clone());
                proptest::prop_assert_eq!(req.body, body.clone());
                Ok::<_, proptest::test_runner::TestCaseError>(())
            }).unwrap();
        }
    }
}

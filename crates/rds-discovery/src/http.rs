//! Minimal HTTP/1.1 wire codec for the discovery directory API.
//!
//! Deliberately small: one request per connection (`Connection:
//! close`), `Content-Length` bodies only, hard caps on head and body
//! size. The directory is internal control-plane infrastructure — both
//! ends are this crate — so a restricted subset is the correct
//! security posture. Parsers are property-tested. This is a private directory
//! profile, not a general HTTP implementation (no chunked/interim responses).
//! Readers may prefetch and discard extra bytes: close the connection after
//! one exchange; never reuse it for pipelined messages.

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

mod headers;

use crate::DiscoveryError;

/// Maximum head size (request/status line + headers) in bytes.
pub const MAX_HEAD: usize = 8 * 1024;
/// Maximum encoded body size in bytes, including JSON envelope overhead.
/// Registry snapshots and revocation feeds must fit this same bound.
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
    let mut stream = BufReader::with_capacity(1024, stream);
    let head = match read_head(&mut stream).await? {
        Some(head) => head,
        None => return Ok(None),
    };
    let text = std::str::from_utf8(&head).map_err(|_| bad("head is not utf-8"))?;
    let text = text
        .strip_suffix("\r\n\r\n")
        .ok_or_else(|| bad("head truncated"))?;
    let mut lines = text.split("\r\n");
    let line = lines.next().ok_or_else(|| bad("empty head"))?;
    let mut parts = line.split(' ');
    let method = parts.next().ok_or_else(|| bad("no method"))?.to_owned();
    let path = parts.next().ok_or_else(|| bad("no path"))?.to_owned();
    let version = parts.next().ok_or_else(|| bad("no version"))?;
    if parts.next().is_some() || !headers::version(version) {
        return Err(bad("malformed request line"));
    }
    headers::target(&method, &path)?;
    let headers = headers::parse(lines)?;
    if version == "HTTP/1.1" && !headers.host {
        return Err(bad("missing host"));
    }
    let mut body = vec![0; headers.length.unwrap_or(0)];
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
    let mut stream = BufReader::with_capacity(1024, stream);
    let head = read_head(&mut stream)
        .await?
        .ok_or_else(|| bad("empty response"))?;
    let text = std::str::from_utf8(&head).map_err(|_| bad("head is not utf-8"))?;
    let text = text
        .strip_suffix("\r\n\r\n")
        .ok_or_else(|| bad("head truncated"))?;
    let mut lines = text.split("\r\n");
    let line = lines.next().ok_or_else(|| bad("empty head"))?;
    let mut parts = line.splitn(3, ' ');
    if !parts.next().is_some_and(headers::version) {
        return Err(bad("malformed status line"));
    }
    let code = parts.next().ok_or_else(|| bad("no status code"))?;
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad("bad status code"));
    }
    let status: u16 = code.parse().map_err(|_| bad("bad status code"))?;
    if !(200..=599).contains(&status) {
        return Err(bad("unsupported status code"));
    }
    let reason = parts
        .next()
        .ok_or_else(|| bad("missing status separator"))?;
    if !headers::field_value(reason) {
        return Err(bad("bad reason phrase"));
    }
    let headers = headers::parse(lines)?;
    let length = match status {
        204 if headers.length.is_some() => return Err(bad("content-length forbidden for 204")),
        204 | 304 => 0,
        _ => headers
            .length
            .ok_or_else(|| bad("response requires content-length"))?,
    };
    let mut body = vec![0; length];
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
    headers::authority(authority)?;
    headers::target(method, path)?;
    if body.len() > MAX_BODY {
        return Err(bad("body too large"));
    }
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if head.len() > MAX_HEAD {
        return Err(bad("head too large"));
    }
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
    if !(200..=599).contains(&resp.status) {
        return Err(bad("unsupported status code"));
    }
    if resp.body.len() > MAX_BODY {
        return Err(bad("body too large"));
    }
    if matches!(resp.status, 204 | 304) && !resp.body.is_empty() {
        return Err(bad("body forbidden for status"));
    }
    let reason = match resp.status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    };
    let head = if matches!(resp.status, 204 | 304) {
        format!(
            "HTTP/1.1 {} {}\r\nConnection: close\r\n\r\n",
            resp.status, reason
        )
    } else {
        format!(
            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
            resp.status,
            reason,
            resp.body.len()
        )
    };
    if head.len() > MAX_HEAD {
        return Err(bad("head too large"));
    }
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
/// zero bytes (an unused connection).
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
        if buf.len() > MAX_HEAD {
            return Err(bad("head too large"));
        }
        if buf.ends_with(b"\r\n\r\n") {
            return Ok(Some(buf));
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

//! One bounded HTTP/1.1 request per connection. No bodies, transfer encodings,
//! upgrades, proxy trust, keep-alive or redirect behavior on the admin surface.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use super::{Counters, Source, Token, snapshot};

pub(super) const MAX_HEADER: usize = 8192;
const CLOSE_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

enum Request {
    Metrics,
    Reject(u16),
}

fn parse(bytes: &[u8], token: &Token) -> Request {
    if bytes.iter().enumerate().any(|(i, byte)| match byte {
        b'\n' => i == 0 || bytes[i - 1] != b'\r',
        b'\r' => bytes.get(i + 1) != Some(&b'\n'),
        _ => false,
    }) {
        return Request::Reject(400);
    }
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut request = httparse::Request::new(&mut headers);
    if !matches!(request.parse(bytes), Ok(httparse::Status::Complete(n)) if n == bytes.len())
        || request.version != Some(1)
    {
        return Request::Reject(400);
    }
    let mut host = false;
    let mut authorization = None;
    let mut length = false;
    for header in request.headers {
        if header.name.eq_ignore_ascii_case("host") {
            if host || header.value.is_empty() {
                return Request::Reject(400);
            }
            host = true;
        } else if header.name.eq_ignore_ascii_case("authorization") {
            if authorization.replace(header.value).is_some() {
                return Request::Reject(400);
            }
        } else if header.name.eq_ignore_ascii_case("content-length") {
            if length || header.value != b"0" {
                return Request::Reject(400);
            }
            length = true;
        } else if ["transfer-encoding", "expect", "upgrade", "origin"]
            .iter()
            .any(|name| header.name.eq_ignore_ascii_case(name))
        {
            return Request::Reject(400);
        }
    }
    if !host {
        return Request::Reject(400);
    }
    if !authorization.is_some_and(|value| token.accepts(value)) {
        return Request::Reject(401);
    }
    if request.method != Some("GET") {
        return Request::Reject(405);
    }
    if request.path != Some("/metrics") {
        return Request::Reject(404);
    }
    Request::Metrics
}

pub(super) async fn serve(
    socket: &mut TcpStream,
    token: &Token,
    source: &Source,
    counters: &Arc<Counters>,
) -> std::io::Result<()> {
    let mut bytes = [0u8; MAX_HEADER];
    let mut used = 0;
    let request = loop {
        if used == bytes.len() {
            break Request::Reject(431);
        }
        let read = socket.read(&mut bytes[used..]).await?;
        if read == 0 {
            return Ok(());
        }
        used += read;
        if bytes[..used].windows(4).any(|end| end == b"\r\n\r\n") {
            break parse(&bytes[..used], token);
        }
    };
    counters.requests.fetch_add(1, Ordering::Relaxed);
    let (status, body) = match request {
        Request::Metrics => {
            counters.scrapes.fetch_add(1, Ordering::Relaxed);
            let mut samples = source();
            // Reserved admin names are never supplied by component sources.
            samples.extend(counters.snapshot());
            match snapshot::render(samples) {
                Some(body) => (200, body),
                None => {
                    counters.snapshot_errors.fetch_add(1, Ordering::Relaxed);
                    (500, String::new())
                }
            }
        }
        Request::Reject(status) => {
            if status == 401 {
                counters.unauthorized.fetch_add(1, Ordering::Relaxed);
            }
            if status == 400 || status == 431 {
                counters.malformed.fetch_add(1, Ordering::Relaxed);
            }
            (status, String::new())
        }
    };
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    };
    let challenge = match status {
        401 => "WWW-Authenticate: Bearer\r\n",
        405 => "Allow: GET\r\n",
        _ => "",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n{challenge}\r\n",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.write_all(body.as_bytes()).await?;
    socket.shutdown().await?;
    // RFC 9112 §9.6: dropping with unread request bytes can reset TCP and
    // erase the response before the peer reads it (observed on macOS).
    // Half-close first, then discard a bounded tail without parsing another
    // request. An uncooperative peer cannot retain a slot past this deadline
    // or the enclosing request deadline; server shutdown can still abort us.
    let _ = tokio::time::timeout(CLOSE_DRAIN_TIMEOUT, async {
        let mut remaining = MAX_HEADER;
        let mut discard = [0u8; 1024];
        while remaining != 0 {
            let limit = remaining.min(discard.len());
            let read = socket.read(&mut discard[..limit]).await?;
            if read == 0 {
                break;
            }
            remaining -= read;
        }
        Ok::<(), std::io::Error>(())
    })
    .await;
    Ok(())
}

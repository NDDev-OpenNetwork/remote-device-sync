use super::{MAX_BODY, bad};
use crate::DiscoveryError;

pub(super) fn token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}
pub(super) fn field_value(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_graphic() || b == b' ' || b == b'\t')
}
pub(super) fn target(method: &str, path: &str) -> Result<(), DiscoveryError> {
    if method.len() > 16 || !token(method) {
        return Err(bad("bad method"));
    }
    if !path.starts_with('/')
        || path.len() > 512
        || !path
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"?#\\".contains(&b))
    {
        return Err(bad("bad path"));
    }
    Ok(())
}
pub(super) fn authority(value: &str) -> Result<(), DiscoveryError> {
    if value.is_empty()
        || value.len() > 2048
        || value.ends_with(':')
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    {
        return Err(bad("invalid host authority"));
    }
    let url =
        url::Url::parse(&format!("http://{value}/")).map_err(|_| bad("invalid host authority"))?;
    if url.host().is_none() || url.port_or_known_default().is_none_or(|port| port == 0) {
        return Err(bad("invalid host authority"));
    }
    Ok(())
}
pub(super) fn version(value: &str) -> bool {
    matches!(value, "HTTP/1.0" | "HTTP/1.1")
}

pub(super) struct Headers {
    pub length: Option<usize>,
    pub host: bool,
}
pub(super) fn parse<'a>(lines: impl Iterator<Item = &'a str>) -> Result<Headers, DiscoveryError> {
    let mut headers = Headers {
        length: None,
        host: false,
    };
    for line in lines {
        let (name, value) = line.split_once(':').ok_or_else(|| bad("bad header"))?;
        // No field-name whitespace, obs-fold, bare CR/LF, C0 or DEL. This
        // private profile uses ASCII fields; UTF-8 remains valid in the body.
        if !token(name) || !field_value(value) {
            return Err(bad("bad header syntax"));
        }
        let value = value.trim_matches([' ', '\t']);
        if name.eq_ignore_ascii_case("content-length") {
            if headers.length.is_some() {
                return Err(bad("duplicate content-length"));
            }
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad("bad content-length"));
            }
            let length: usize = value.parse().map_err(|_| bad("bad content-length"))?;
            if length > MAX_BODY {
                return Err(bad("body too large"));
            }
            headers.length = Some(length);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(bad("transfer-encoding unsupported"));
        } else if name.eq_ignore_ascii_case("host") {
            if headers.host {
                return Err(bad("duplicate host"));
            }
            authority(value)?;
            headers.host = true;
        } else if name.eq_ignore_ascii_case("content-encoding")
            && !value.eq_ignore_ascii_case("identity")
        {
            return Err(bad("content-encoding unsupported"));
        } else if name.eq_ignore_ascii_case("expect") {
            return Err(bad("expectation unsupported"));
        }
    }
    Ok(headers)
}

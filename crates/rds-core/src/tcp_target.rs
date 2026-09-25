//! Canonical TCP service destinations shared by configuration frontends.
//! Parsing performs no DNS lookup and never opens a socket.

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TcpTarget {
    host: String,
    port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TcpTargetError(&'static str);

impl TcpTarget {
    /// Construct from a host without brackets and a nonzero port. Hostnames
    /// use ASCII (including local underscore aliases); use punycode for IDNs.
    /// Scoped IPv6 needs a future explicit interface representation.
    pub fn new(host: impl AsRef<str>, port: u16) -> Result<Self, TcpTargetError> {
        let host = host.as_ref();
        if port == 0 {
            return Err(TcpTargetError("TCP destination port must be 1..65535"));
        }
        let without_root_dot = host.strip_suffix('.').unwrap_or(host);
        if without_root_dot.is_empty() || without_root_dot.len() > 253 {
            return Err(TcpTargetError(
                "TCP destination host must be 1..253 bytes before an optional final DNS dot",
            ));
        }
        let host = match host.parse::<IpAddr>() {
            Ok(ip) => {
                let ip = ip.to_canonical();
                if ip.is_unspecified()
                    || ip.is_multicast()
                    || matches!(ip, IpAddr::V4(v4) if v4.is_broadcast())
                {
                    return Err(TcpTargetError("TCP destination must be a unicast address"));
                }
                ip.to_string()
            }
            Err(_) => {
                if !host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
                    || host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                    || host
                        .trim_end_matches('.')
                        .split('.')
                        .any(|label| label.is_empty() || label.len() > 63)
                    || host.ends_with("..")
                {
                    return Err(TcpTargetError(
                        "invalid TCP hostname or IP address; IPv6 zones are not supported",
                    ));
                }
                host.to_ascii_lowercase()
            }
        };
        Ok(Self { host, port })
    }

    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn into_parts(self) -> (String, u16) {
        (self.host, self.port)
    }
}

impl FromStr for TcpTarget {
    type Err = TcpTargetError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        // Bound before splitting/parsing or allocating a normalized host.
        if value.len() > 261 {
            return Err(TcpTargetError("TCP destination exceeds its size limit"));
        }
        let (host, port) = if let Some(rest) = value.strip_prefix('[') {
            let (host, tail) = rest
                .split_once(']')
                .ok_or(TcpTargetError("unterminated IPv6 address"))?;
            host.parse::<Ipv6Addr>()
                .map_err(|_| TcpTargetError("brackets require an IPv6 literal without a zone"))?;
            (
                host,
                tail.strip_prefix(':')
                    .ok_or(TcpTargetError("expected [IPv6]:port"))?,
            )
        } else {
            let (host, port) = value
                .rsplit_once(':')
                .ok_or(TcpTargetError("expected host:port or [IPv6]:port"))?;
            if host.contains(':') {
                return Err(TcpTargetError("IPv6 TCP destinations require [IPv6]:port"));
            }
            (host, port)
        };
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(TcpTargetError("TCP destination port must be 1..65535"));
        }
        let port = port
            .parse()
            .map_err(|_| TcpTargetError("TCP destination port must be 1..65535"))?;
        Self::new(host, port)
    }
}

impl TryFrom<String> for TcpTarget {
    type Error = TcpTargetError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<TcpTarget> for String {
    fn from(value: TcpTarget) -> Self {
        value.to_string()
    }
}

impl fmt::Display for TcpTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_targets_roundtrip_without_dns() {
        for (input, host, port) in [
            ("127.0.0.1:22", "127.0.0.1", 22),
            ("LOCALHOST:0022", "localhost", 22),
            ("SSH.Example.:65535", "ssh.example.", 65535),
            ("local_alias:2222", "local_alias", 2222),
            ("[0:0:0:0:0:0:0:1]:22", "::1", 22),
            ("[2001:db8::1]:2222", "2001:db8::1", 2222),
            ("[::ffff:127.0.0.1]:22", "127.0.0.1", 22),
        ] {
            let target: TcpTarget = input.parse().unwrap();
            assert_eq!(target.host(), host);
            assert_eq!(target.port(), port);
            assert_eq!(target.to_string().parse::<TcpTarget>().unwrap(), target);
            assert_eq!(target.into_parts(), (host.into(), port));
        }
        let longest = format!(
            "{}.{}.{}.{}",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(61)
        );
        assert_eq!(longest.len(), 253);
        assert!(format!("{longest}.:65535").parse::<TcpTarget>().is_ok());
        assert!(format!("{longest}e:22").parse::<TcpTarget>().is_err());
    }

    #[test]
    fn ambiguous_malformed_and_non_unicast_targets_fail() {
        for input in [
            "",
            "host",
            ":22",
            "host:",
            "host:0",
            "host:+22",
            "host:-22",
            "host:65536",
            "host:22junk",
            "host:22 ",
            " host:22",
            "h\0ost:22",
            "::1:22",
            "[::1]22",
            "[::1]:22:3",
            "[::1:22",
            "[localhost]:22",
            "[fe80::1%1]:22",
            "user@host:22",
            "host/path:22",
            "127.1:22",
            "999.0.0.1:22",
            "0.0.0.0:22",
            "[::]:22",
            "224.0.0.1:22",
            "255.255.255.255:22",
            "[::ffff:0.0.0.0]:22",
            "[::ffff:224.0.0.1]:22",
            "[::ffff:255.255.255.255]:22",
            "[ff02::1]:22",
            "..:22",
            "a..b:22",
            "a...:22",
            "💻.example:22",
        ] {
            assert!(input.parse::<TcpTarget>().is_err(), "accepted {input:?}");
        }
        assert!(TcpTarget::new("host", 0).is_err());
        assert!(TcpTarget::new("[::1]", 22).is_err());
        assert!(
            format!("{}:22", "a".repeat(64))
                .parse::<TcpTarget>()
                .is_err()
        );
        assert!(
            format!("{}:22", "a.".repeat(200))
                .parse::<TcpTarget>()
                .is_err()
        );
    }
}

//! Versioned local control protocol. This is not a remote service ALPN.
use serde::{Deserialize, Serialize};

pub const VERSION: u16 = 3;
pub const MAX_SESSIONS: usize = 32;
pub const TCP_CHUNK: usize = 16 * 1024;

/// Local TCP bodies distinguish byte-direction FIN from caller departure.
/// Never Debug: application bytes can contain secrets.
#[derive(Serialize, Deserialize)]
pub enum TcpFrame {
    Data(Vec<u8>),
    Finish,
}

/// Random, process-lifetime handle. Never aliases a session after agent restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(pub [u8; 16]);

impl TryFrom<String> for SessionId {
    type Error = ErrorCode;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<SessionId> for String {
    fn from(value: SessionId) -> Self {
        value.to_string()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl std::str::FromStr for SessionId {
    type Err = ErrorCode;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ErrorCode::InvalidRequest);
        }
        let mut bytes = [0; 16];
        for (byte, pair) in bytes.iter_mut().zip(s.as_bytes().as_chunks::<2>().0) {
            let digit = |b: u8| {
                if b.is_ascii_digit() {
                    b - b'0'
                } else {
                    b.to_ascii_lowercase() - b'a' + 10
                }
            };
            *byte = digit(pair[0]) * 16 + digit(pair[1]);
        }
        Ok(Self(bytes))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Request {
    pub version: u16,
    pub command: Command,
}

/// Deliberately not Debug: a connect command can carry a credential.
#[derive(Clone, Serialize, Deserialize)]
pub enum Command {
    List,
    Connect {
        target: String,
        grant: Option<Box<crate::grant::Grant>>,
    },
    Select {
        session: SessionId,
    },
    Disconnect {
        session: SessionId,
    },
    Ping {
        session: Option<SessionId>,
        nonce: u64,
    },
    Info {
        session: Option<SessionId>,
    },
    /// An Opened reply is followed by framed TCP data and explicit direction FIN.
    OpenTcp {
        session: Option<SessionId>,
        target: crate::TcpTarget,
    },
    /// Current addresses of the already-bound agent endpoint; no relay wait.
    Ticket,
    /// Extend one explicitly pinned grant-backed session without reconnecting.
    Renew {
        session: SessionId,
        grant: Box<crate::grant::Grant>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Connecting,
    Connected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub peer: String,
    pub status: Status,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub instance: SessionId,
    pub generation: u64,
    pub endpoint: String,
    pub selected: Option<SessionId>,
    pub sessions: Vec<Session>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub version: u16,
    pub result: Result<Reply, ErrorCode>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reply {
    Snapshot(Snapshot),
    Connected(SessionId),
    Done,
    Pong {
        session: SessionId,
        micros: u64,
    },
    Info {
        session: SessionId,
        info: crate::AgentInfo,
    },
    Opened(SessionId),
    Ticket(String),
}

/// Stable reasons, with no upstream error, address, path or credential payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ErrorCode {
    #[error("unsupported local protocol version")]
    Version,
    #[error("invalid local request")]
    InvalidRequest,
    #[error("session or worker capacity reached")]
    Capacity,
    #[error("connection to this peer is already pending")]
    Busy,
    #[error("peer already connected with different credentials; disconnect it first")]
    CredentialConflict,
    #[error("session is no longer available")]
    NotFound,
    #[error("no device selected; connect or select a session first")]
    NoSelection,
    #[error("target resolution failed; use a pinned ticket or key, or configure directory trust")]
    Resolve,
    #[error("peer connection or authorization failed")]
    Connect,
    #[error("remote operation failed or was denied")]
    Remote,
    #[error("local request deadline exceeded")]
    Timeout,
    #[error("manager is stopping")]
    Stopped,
    #[error("manager state unavailable")]
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_and_validated_tcp_targets_round_trip() {
        let id = SessionId([0xa5; 16]);
        assert_eq!(id.to_string().parse::<SessionId>().unwrap(), id);
        assert_eq!(
            id.to_string().to_uppercase().parse::<SessionId>().unwrap(),
            id
        );
        for invalid in [
            "",
            "a",
            "gggggggggggggggggggggggggggggggg",
            "éééééééééééééééé",
        ] {
            assert!(invalid.parse::<SessionId>().is_err());
        }
        let request = Request {
            version: VERSION,
            command: Command::OpenTcp {
                session: Some(id),
                target: "[::ffff:127.0.0.1]:22".parse().unwrap(),
            },
        };
        let bytes = postcard::to_stdvec(&request).unwrap();
        let decoded: Request = postcard::from_bytes(&bytes).unwrap();
        match decoded.command {
            Command::OpenTcp { session, target } => {
                assert_eq!(session, Some(id));
                assert_eq!(target.to_string(), "127.0.0.1:22");
            }
            _ => panic!("wrong variant"),
        }
        for invalid in ["host:0", ":22", "[::]:22", "host:nope"] {
            let encoded = postcard::to_stdvec(invalid).unwrap();
            assert!(postcard::from_bytes::<crate::TcpTarget>(&encoded).is_err());
        }
    }

    proptest::proptest! {
        #[test]
        fn local_decoders_never_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..1024), text in ".*") {
            let _ = postcard::from_bytes::<Request>(&bytes);
            let _ = postcard::from_bytes::<TcpFrame>(&bytes);
            let _ = postcard::from_bytes::<Response>(&bytes);
            let _ = text.parse::<SessionId>();
        }
    }
}

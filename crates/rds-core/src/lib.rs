//! Protocol types shared by every rds peer.
//!
//! Layout: one QUIC connection per peer pair (`ALPN = rds/0`), every
//! bi-directional stream opens with a length-prefixed postcard
//! [`StreamHello`], every uni-directional stream that carries a video frame
//! opens with a [`FrameHeader`]. Control messages on an established desktop
//! control stream are [`DesktopControl`] frames.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Capability grants (WS4): signed service-scope tokens presented on
/// the connection's first stream.
pub mod grant;
pub mod local;
/// Owned relay protocol wire types (ALPN `rds-relay/0`).
pub mod relay;
mod tcp_target;
pub use tcp_target::{TcpTarget, TcpTargetError};

/// ALPN negotiated for all rds traffic.
pub const ALPN: &[u8] = b"rds/0";

/// Wire protocol version. Peers refuse mismatched majors.
/// v2: FrameHeader carries capture/encode/send timestamps, InputEvent
/// carries metadata, control stream gains heartbeat + input acks.
/// v3: every uni-directional stream opens with a [`UniHello`] tag so a
/// single per-connection demux can route it — v2 consumers each called
/// `accept_uni` directly and could steal each other's streams.
pub const PROTOCOL_VERSION: u16 = 3;

/// Upper bound for a serialized greeting, guard against abusive peers.
pub const MAX_MESSAGE_LEN: u32 = 64 * 1024;

/// First frame on every uni-directional stream (v3): routes the stream
/// to the service that owns it. The accepting side runs one
/// `accept_uni` demux per connection and hands each stream to the
/// consumer registered for its tag — two services on one connection can
/// no longer consume each other's streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UniHello {
    /// Desktop video frame stream: a [`FrameHeader`] then the encoded
    /// payload follow.
    Desktop,
    /// Sync chunk stream: `SyncMsg` frames (`ChunkSet`, `ChunkHdr` +
    /// bytes, `SetDone`) follow.
    Sync,
    /// Audio packet stream: [`AudioFrame`] records follow (codec
    /// support lands in v0.3).
    Audio,
    /// Isolated file-transfer route. Never reuse an ID on a connection.
    SyncTransfer { id: [u8; 16] },
}

/// First frame on every bi-directional stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamHello {
    /// Round-trip probe; the peer echoes `nonce` back verbatim.
    Ping { nonce: u64 },
    /// Ask for peer metadata.
    Info,
    /// Splice this stream to a TCP socket on the serving side.
    TcpConnect { host: String, port: u16 },
    /// Open a desktop session control channel.
    Desktop(DesktopHello),
    /// Transfer a file or directory (WS6 sync protocol follows inside).
    Sync,
    /// Open an audio channel: one uni stream of [`AudioFrame`] records
    /// server→client on its own clock (codec support lands in v0.3).
    Audio(AudioHello),
    /// Present a capability [`grant::Grant`]. Must be the first stream
    /// on the connection when the agent runs in grant mode: every
    /// service received before authorization starts is refused. Services that
    /// overlap its reply/commit transaction wait for completion with a deadline.
    Authz(grant::Grant),
    /// Signed lease revision for this connection; same identity and exact scope.
    RenewAuthz(grant::Grant),
    /// File transfer with an isolated uni-stream route. Additive extension;
    /// older agents reject this greeting before filesystem operations.
    SyncTransfer { id: [u8; 16] },
}

/// Answer to a [`StreamHello`], sent before any service payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HelloAck {
    /// Request accepted; the service payload follows on the same stream.
    Ok,
    /// Request rejected; the stream ends after this frame.
    Error { message: String },
    /// Answer to [`StreamHello::Info`].
    Info(AgentInfo),
    /// Answer to [`StreamHello::Desktop`].
    Desktop(DesktopCaps),
}

/// What the serving side offers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub protocol: u16,
    pub version: String,
    pub hostname: Option<String>,
    pub services: Vec<ServiceKind>,
    pub desktop: Option<DesktopCaps>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceKind {
    Ping,
    Info,
    Tcp,
    Desktop,
    /// Bulk file transfer (WS6).
    Sync,
    /// Audio forwarding (v0.3 codec; wire shape reserved in v2).
    Audio,
    /// Download files from the agent's configured sync root.
    SyncRead,
    /// Upload files to the agent's configured sync root.
    SyncWrite,
    /// View a desktop without injecting input.
    DesktopView,
    /// Input modifier: requires `DesktopView` to open a session.
    DesktopControl,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopHello {
    /// Display index to capture.
    pub display: u32,
    /// Upper bound on produced frames per second.
    pub max_fps: u32,
    /// Requested codec.
    pub codec: Codec,
    /// Measurement mode: server acks each injected input event.
    pub input_acks: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopCaps {
    pub displays: Vec<DisplayInfo>,
    pub codecs: Vec<Codec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub index: u32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    /// H.264 Annex-B, constrained baseline, no B-frames.
    H264,
}

/// Header at the start of every uni-directional video frame stream (v2).
///
/// All timestamps are milliseconds on the producer's session clock:
/// `capture_ts_ms` when the frame was captured, `encode_done_ts_ms` when
/// the encoder returned it, `send_ts_ms` when it was handed to the
/// transport — together they split pipeline delay from network delay,
/// which is what the latency budget is measured against.
///
/// The receiver resets a frame stream when a newer `seq` has already been
/// fully received: stale streams cost no further bandwidth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameHeader {
    pub seq: u64,
    pub capture_ts_ms: u64,
    pub encode_done_ts_ms: u64,
    pub send_ts_ms: u64,
    pub keyframe: bool,
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
}

/// Client→server messages on the desktop control stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DesktopControl {
    /// Ask for a fresh IDR (after join or packet loss).
    RequestIdr,
    /// Ask the encoder to aim for this bitrate in bits per second.
    SetBitrate(u32),
    /// One input event to inject on the serving side.
    Input(InputEvent),
    /// Liveness probe; the server echoes it as [`DesktopEvent::Heartbeat`].
    /// Lets the viewer measure control-plane RTT under video backlog.
    Heartbeat { seq: u64, ts_ms: u64 },
}

/// Server→client messages on the desktop control stream (v2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DesktopEvent {
    /// One input event was injected (sent when `DesktopHello::input_acks`).
    InputAck { seq: u64, handled_ts_ms: u64 },
    /// Echo of [`DesktopControl::Heartbeat`].
    Heartbeat { seq: u64, ts_ms: u64 },
}

/// One input event plus the metadata the serving side needs to route and
/// the viewer needs to correlate acks (v2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputEvent {
    /// Per-session sequence number, assigned by the viewer.
    pub seq: u64,
    /// When the viewer produced the event, ms on its own clock.
    pub event_ts_ms: u64,
    /// Display the event targets.
    pub display_id: u32,
    pub kind: InputKind,
}

/// The input action itself; carried inside [`InputEvent`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputKind {
    /// Linux evdev key code, pressed.
    KeyDown { code: u32 },
    /// Linux evdev key code, released.
    KeyUp { code: u32 },
    /// Absolute pointer position in display coordinates.
    PointerMove { x: f64, y: f64 },
    /// Relative pointer motion.
    PointerMotion { dx: f64, dy: f64 },
    /// evdev button code.
    PointerButton { button: i32, pressed: bool },
    /// Scroll in fractional wheel steps: positive x is left, positive y is up.
    Scroll { dx: f64, dy: f64 },
}

/// Audio channel negotiation (`StreamHello::Audio`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioHello {
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioCodec {
    /// Opus in Ogg-less framing; one [`AudioFrame`] record per packet.
    Opus,
}

/// One audio packet on the session's uni audio stream. `capture_ts_ms`
/// is on the audio device's own clock, independent of the video clock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFrame {
    pub seq: u64,
    pub capture_ts_ms: u64,
    /// Samples per channel in this packet.
    pub samples: u32,
    pub data: Vec<u8>,
}

/// Serialize `msg` as postcard and write it with a big-endian u32 length.
pub async fn write_frame<W, M>(writer: &mut W, msg: &M) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
    M: Serialize,
{
    let body = postcard::to_stdvec(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len: u32 = body
        .len()
        .try_into()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"))?;
    if len > MAX_MESSAGE_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame exceeds MAX_MESSAGE_LEN",
        ));
    }
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&body).await
}

/// Read one length-prefixed postcard frame.
pub async fn read_frame<R, M>(reader: &mut R) -> std::io::Result<M>
where
    R: AsyncRead + Unpin,
    M: for<'de> Deserialize<'de>,
{
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_MESSAGE_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame exceeds MAX_MESSAGE_LEN",
        ));
    }
    let mut body = vec![0u8; len as usize];
    reader.read_exact(&mut body).await?;
    let (message, remaining) = postcard::take_from_bytes(&body)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if !remaining.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "trailing frame payload",
        ));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    #[test]
    fn service_wire_tags_are_append_only_and_old_decoders_refuse_new_scopes() {
        use super::ServiceKind::*;
        #[derive(serde::Deserialize)]
        enum LegacyService {
            Ping,
            Info,
            Tcp,
            Desktop,
            Sync,
            Audio,
        }
        for (tag, kind) in [
            Ping,
            Info,
            Tcp,
            Desktop,
            Sync,
            Audio,
            SyncRead,
            SyncWrite,
            DesktopView,
            DesktopControl,
        ]
        .into_iter()
        .enumerate()
        {
            let bytes = postcard::to_stdvec(&kind).unwrap();
            assert_eq!(bytes, vec![tag as u8]);
            assert_eq!(postcard::from_bytes::<ServiceKind>(&bytes).unwrap(), kind);
            assert_eq!(
                postcard::from_bytes::<LegacyService>(&bytes).is_ok(),
                tag < 6
            );
        }
    }
    use super::*;

    #[test]
    fn isolated_sync_appends_tags_and_legacy_decoders_refuse_the_extension() {
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        enum LegacyHello {
            Ping { nonce: u64 },
            Info,
            TcpConnect { host: String, port: u16 },
            Desktop(DesktopHello),
            Sync,
            Audio(AudioHello),
            Authz(grant::Grant),
            RenewAuthz(grant::Grant),
        }
        #[derive(serde::Deserialize)]
        enum LegacyUni {
            Desktop,
            Sync,
            Audio,
        }
        assert_eq!(postcard::to_stdvec(&StreamHello::Sync).unwrap(), [4]);
        assert_eq!(postcard::to_stdvec(&UniHello::Sync).unwrap(), [1]);
        let hello = postcard::to_stdvec(&StreamHello::SyncTransfer { id: [42; 16] }).unwrap();
        let uni = postcard::to_stdvec(&UniHello::SyncTransfer { id: [42; 16] }).unwrap();
        assert_eq!(hello, [vec![8], vec![42; 16]].concat());
        assert_eq!(uni, [vec![3], vec![42; 16]].concat());
        assert!(postcard::from_bytes::<LegacyHello>(&hello).is_err());
        assert!(postcard::from_bytes::<LegacyUni>(&uni).is_err());
    }

    #[tokio::test]
    async fn frame_rejects_trailing_postcard_payload() {
        let message = StreamHello::TcpConnect {
            host: "127.0.0.1".into(),
            port: 22,
        };
        let mut body = postcard::to_stdvec(&message).unwrap();
        body.extend_from_slice(&[0, 1]);
        let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(&body);
        assert!(
            read_frame::<_, StreamHello>(&mut bytes.as_slice())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn frame_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let hello = StreamHello::TcpConnect {
            host: "127.0.0.1".into(),
            port: 22,
        };
        write_frame(&mut a, &hello).await.unwrap();
        match read_frame::<_, StreamHello>(&mut b).await.unwrap() {
            StreamHello::TcpConnect { host, port } => {
                assert_eq!(host, "127.0.0.1");
                assert_eq!(port, 22);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&(MAX_MESSAGE_LEN + 1).to_be_bytes())
            .await
            .unwrap();
        assert!(read_frame::<_, StreamHello>(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn frame_header_v2_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let header = FrameHeader {
            seq: 41,
            capture_ts_ms: 1_000,
            encode_done_ts_ms: 1_004,
            send_ts_ms: 1_005,
            keyframe: true,
            codec: Codec::H264,
            width: 1920,
            height: 1080,
        };
        write_frame(&mut a, &header).await.unwrap();
        let got = read_frame::<_, FrameHeader>(&mut b).await.unwrap();
        assert_eq!(got.seq, 41);
        assert_eq!(got.capture_ts_ms, 1_000);
        assert_eq!(got.encode_done_ts_ms, 1_004);
        assert_eq!(got.send_ts_ms, 1_005);
        assert!(got.keyframe);
        assert_eq!((got.width, got.height), (1920, 1080));
    }

    #[tokio::test]
    async fn input_event_v2_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let msg = DesktopControl::Input(InputEvent {
            seq: 7,
            event_ts_ms: 42_000,
            display_id: 1,
            kind: InputKind::PointerButton {
                button: 272,
                pressed: true,
            },
        });
        write_frame(&mut a, &msg).await.unwrap();
        match read_frame::<_, DesktopControl>(&mut b).await.unwrap() {
            DesktopControl::Input(ev) => {
                assert_eq!(ev.seq, 7);
                assert_eq!(ev.display_id, 1);
                assert!(matches!(
                    ev.kind,
                    InputKind::PointerButton {
                        button: 272,
                        pressed: true
                    }
                ));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn heartbeat_roundtrip_both_directions() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        write_frame(&mut a, &DesktopControl::Heartbeat { seq: 3, ts_ms: 9 })
            .await
            .unwrap();
        match read_frame::<_, DesktopControl>(&mut b).await.unwrap() {
            DesktopControl::Heartbeat { seq, ts_ms } => {
                assert_eq!((seq, ts_ms), (3, 9));
            }
            other => panic!("unexpected {other:?}"),
        }
        write_frame(&mut b, &DesktopEvent::Heartbeat { seq: 3, ts_ms: 9 })
            .await
            .unwrap();
        match read_frame::<_, DesktopEvent>(&mut a).await.unwrap() {
            DesktopEvent::Heartbeat { seq, ts_ms } => {
                assert_eq!((seq, ts_ms), (3, 9));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    proptest::proptest! {
        /// The frame-stream demux path must never panic on arbitrary
        /// bytes: length-prefix plus postcard decode is bounded and
        /// error-returning on anything malformed.
        #[test]
        fn frame_header_decode_never_panics(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256)) {
            let mut cur = std::io::Cursor::new(bytes);
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _ = rt.block_on(read_frame::<_, FrameHeader>(&mut cur));
        }

        /// Same for the control-stream demux: every enum pulled off the
        /// wire is decoded under the length bound.
        #[test]
        fn control_demux_never_panics(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256)) {
            let mut cur = std::io::Cursor::new(bytes.clone());
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _ = rt.block_on(read_frame::<_, DesktopControl>(&mut cur));
            let mut cur = std::io::Cursor::new(bytes);
            let _ = rt.block_on(read_frame::<_, StreamHello>(&mut cur));
        }
    }
}

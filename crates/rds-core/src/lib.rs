//! Protocol types shared by every rds peer.
//!
//! Layout: one QUIC connection per peer pair (`ALPN = rds/0`), every
//! bi-directional stream opens with a length-prefixed postcard
//! [`StreamHello`], every uni-directional stream that carries a video frame
//! opens with a [`FrameHeader`]. Control messages on an established desktop
//! control stream are [`DesktopControl`] frames.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Owned relay protocol wire types (ALPN `rds-relay/0`).
pub mod relay;

/// ALPN negotiated for all rds traffic.
pub const ALPN: &[u8] = b"rds/0";

/// Wire protocol version. Peers refuse mismatched majors.
pub const PROTOCOL_VERSION: u16 = 1;

/// Upper bound for a serialized greeting, guard against abusive peers.
pub const MAX_MESSAGE_LEN: u32 = 64 * 1024;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopHello {
    /// Display index to capture.
    pub display: u32,
    /// Upper bound on produced frames per second.
    pub max_fps: u32,
    /// Requested codec.
    pub codec: Codec,
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

/// Header at the start of every uni-directional video frame stream.
///
/// The receiver resets a frame stream when a newer `seq` has already been
/// fully received: stale streams cost no further bandwidth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameHeader {
    pub seq: u64,
    pub pts_ms: u64,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
    pub codec: Codec,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
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
    /// High-resolution scroll deltas.
    Scroll { dx: f64, dy: f64 },
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
    postcard::from_bytes(&body).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

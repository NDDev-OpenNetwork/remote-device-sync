//! Async frame I/O for the RDS wire types.
//!
//! `rds-core` owns the types and the 64 KiB bound but stays a
//! runtime-free leaf; the tokio read/write half of postcard framing
//! lives here so every peer shares one bounded implementation.

use rds_core::MAX_MESSAGE_LEN;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Short lifecycle requests and desktop input/heartbeats precede media.
pub const CONTROL_STREAM_PRIORITY: i32 = i32::MAX;
/// Media precedes default-priority bulk streams, while control stays responsive.
pub const MEDIA_STREAM_PRIORITY: i32 = i32::MAX / 2;

/// Apply the shared control class in either direction. Streaming TCP/sync
/// bodies retain their existing priority; their greeting does not promote
/// the lifetime of a bulk transfer into the control class.
pub fn prioritize_control(
    send: &crate::SendStream,
    hello: &rds_core::StreamHello,
) -> Result<(), crate::ClosedStream> {
    if matches!(
        hello,
        rds_core::StreamHello::Ping { .. }
            | rds_core::StreamHello::Info
            | rds_core::StreamHello::Authz(_)
            | rds_core::StreamHello::RenewAuthz(_)
            | rds_core::StreamHello::Desktop(_)
            | rds_core::StreamHello::DesktopV2 { .. }
            | rds_core::StreamHello::DesktopV3 { .. }
            | rds_core::StreamHello::DesktopV4 { .. }
    ) {
        send.set_priority(CONTROL_STREAM_PRIORITY)?;
    }
    Ok(())
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
    use super::*;
    use rds_core::{
        Codec, DesktopControl, DesktopEvent, FrameHeader, InputEvent, InputKind, StreamHello,
    };

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

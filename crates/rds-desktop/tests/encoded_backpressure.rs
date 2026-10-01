//! A slow local IPC consumer must not lose compressed H.264 references.
#![cfg(feature = "x11")]
use bytes::Bytes;
use rds_core::{Codec, DesktopHello, FrameHeader, HelloAck, StreamHello};
use rds_desktop::client::{DesktopSession, SessionOpts};
use rds_desktop::codec::openh264::{H264Decoder, H264Encoder};
use rds_desktop::{
    Decoder, EncodedFrame, Encoder, FrameProducer, Produced, ProducerControls, RawFrame,
    SessionClock, SessionConfig, serve_desktop_with,
};
use rds_net::{EndpointConfig, read_frame, write_frame};
use std::sync::atomic::Ordering;
use std::time::Duration;
struct Burst {
    encoder: H264Encoder,
    stop: std::sync::mpsc::Receiver<()>,
}
impl FrameProducer for Burst {
    fn produce(
        &mut self,
        seq: u64,
        controls: &ProducerControls,
        clock: &SessionClock,
    ) -> Option<Produced> {
        if seq > 8 {
            let _ = self.stop.recv_timeout(Duration::from_secs(3));
            return None;
        }
        std::thread::sleep(Duration::from_millis(8));
        if controls.idr.swap(false, Ordering::Relaxed) {
            self.encoder.request_idr();
        }
        let pixels = (0..64 * 64)
            .flat_map(|p| {
                [
                    (p % 251) as u8,
                    (p % 191) as u8,
                    if p < 16 {
                        (seq * 17) as u8
                    } else {
                        (p % 97) as u8
                    },
                    255,
                ]
            })
            .collect::<Vec<_>>();
        let frame = self
            .encoder
            .encode(&RawFrame {
                width: 64,
                height: 64,
                stride: 256,
                data: Bytes::from(pixels),
            })
            .unwrap();
        assert_eq!(
            frame.keyframe,
            seq == 0,
            "fixture must retain a continuous delta reference chain"
        );
        assert!(
            !frame.data.is_empty(),
            "fixture must emit actual references"
        );
        Some(Produced {
            header: FrameHeader {
                seq,
                capture_ts_ms: clock.now_ms(),
                encode_done_ts_ms: clock.now_ms(),
                send_ts_ms: 0,
                keyframe: frame.keyframe,
                codec: Codec::H264,
                width: 64,
                height: 64,
            },
            payload: frame.data,
        })
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_encoded_consumer_preserves_every_reference_and_remains_decodable() {
    let config = EndpointConfig {
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    };
    let server = rds_net::bind_endpoint(config.clone()).await.unwrap();
    let client = rds_net::bind_endpoint(config).await.unwrap();
    let (stop, stopped) = std::sync::mpsc::channel();
    let serving = tokio::spawn({
        let server = server.clone();
        async move {
            let conn = server.accept().await.unwrap().await.unwrap();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let StreamHello::DesktopV2 { session, hello } = read_frame(&mut recv).await.unwrap()
            else {
                panic!("expected tagged desktop");
            };
            write_frame(
                &mut send,
                &HelloAck::Desktop(rds_core::DesktopCaps {
                    displays: vec![],
                    codecs: vec![Codec::H264],
                }),
            )
            .await
            .unwrap();
            let _ = serve_desktop_with(
                conn,
                send,
                recv,
                hello,
                SessionConfig {
                    view_only: true,
                    producer: Some(Box::new(Burst {
                        encoder: H264Encoder::new(4_000_000, 60.).unwrap(),
                        stop: stopped,
                    })),
                    frame_route: Some(rds_core::UniHello::DesktopFrames { id: session }),
                    ..Default::default()
                },
            )
            .await;
        }
    });
    let conn = client.connect(server.addr(), rds_core::ALPN).await.unwrap();
    let mut session = DesktopSession::connect_opts(
        &conn,
        DesktopHello {
            display: 0,
            max_fps: 60,
            codec: Codec::H264,
            input_acks: false,
        },
        SessionOpts {
            session: Some(rand::random()),
            relay_encoded: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let first = tokio::time::timeout(
        Duration::from_secs(2),
        session.encoded.as_mut().unwrap().recv(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(first.header.seq, 0);
    assert!(first.header.keyframe);
    let mut decoder = H264Decoder::new().unwrap();
    assert!(
        decoder
            .decode(&EncodedFrame {
                codec: first.header.codec,
                data: first.payload,
                keyframe: first.header.keyframe
            })
            .unwrap()
            .is_some()
    );
    // Model a blocked IPC write/UI decoder while the wire receiver continues.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut observed = Vec::new();
    let mut decoded = 0;
    for expected in 1..=8 {
        let frame = tokio::time::timeout(
            Duration::from_secs(2),
            session.encoded.as_mut().unwrap().recv(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            frame.header.seq, expected,
            "compressed reference was evicted before local decode"
        );
        observed.push(frame.header.seq);
        if decoder
            .decode(&EncodedFrame {
                codec: frame.header.codec,
                data: frame.payload,
                keyframe: frame.header.keyframe,
            })
            .unwrap()
            .is_some()
        {
            decoded += 1;
        }
    }
    stop.send(()).unwrap();
    drop(session);
    conn.close(0u32.into(), b"fixture complete");
    tokio::time::timeout(Duration::from_secs(2), serving)
        .await
        .unwrap()
        .unwrap();
    server.close().await;
    client.close().await;
    assert_eq!(
        observed,
        (1..=8).collect::<Vec<_>>(),
        "slow IPC evicted an encoded reference"
    );
    assert_eq!(
        decoded, 8,
        "preserved reference stream must remain decodable"
    );
}

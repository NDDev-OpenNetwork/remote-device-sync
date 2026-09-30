//! Codec-skipped input is not a lost encoded reference. Exercise the real
//! serving loop, tagged QUIC frames and relay receiver without a display.
use bytes::Bytes;
use rds_core::{Codec, DesktopHello, FrameHeader, HelloAck, StreamHello};
use rds_desktop::client::{DesktopSession, SessionOpts};
use rds_desktop::{
    FrameProducer, Produced, ProducerControls, SessionClock, SessionConfig, serve_desktop_with,
};
use rds_net::{EndpointConfig, read_frame, write_frame};
use std::time::Duration;

struct SkipOnce {
    calls: usize,
    release: std::sync::mpsc::Receiver<()>,
}
impl FrameProducer for SkipOnce {
    fn preserves_reference(&self) -> bool {
        self.calls == 2
    }
    fn produce(
        &mut self,
        seq: u64,
        controls: &ProducerControls,
        clock: &SessionClock,
    ) -> Option<Produced> {
        self.calls += 1;
        if self.calls > 3 {
            let _ = self.release.recv_timeout(Duration::from_secs(3));
            return None;
        }
        // Separate writes to distinguish a harmless codec skip from writer
        // queue collapse; both native production and this fixture are bounded.
        std::thread::sleep(Duration::from_millis(30));
        let keyframe = controls
            .idr
            .swap(false, std::sync::atomic::Ordering::Relaxed);
        Some(Produced {
            header: FrameHeader {
                seq,
                capture_ts_ms: clock.now_ms(),
                encode_done_ts_ms: clock.now_ms(),
                send_ts_ms: 0,
                keyframe,
                codec: Codec::H264,
                width: 64,
                height: 64,
            },
            payload: if self.calls == 2 {
                Bytes::new()
            } else {
                Bytes::from_static(b"encoded")
            },
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skipped_encode_keeps_sequence_and_does_not_force_an_idr() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let config = EndpointConfig {
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        };
        let server = rds_net::bind_endpoint(config.clone()).await.unwrap();
        let client = rds_net::bind_endpoint(config).await.unwrap();
        let (release, held) = std::sync::mpsc::channel();
        let serving = tokio::spawn({
            let server = server.clone();
            async move {
                let conn = server.accept().await.unwrap().await.unwrap();
                let (mut send, mut recv) = conn.accept_bi().await.unwrap();
                let StreamHello::DesktopV2 { session, hello } =
                    read_frame(&mut recv).await.unwrap()
                else {
                    panic!("expected isolated desktop");
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
                serve_desktop_with(
                    conn,
                    send,
                    recv,
                    hello,
                    SessionConfig {
                        view_only: true,
                        producer: Some(Box::new(SkipOnce {
                            calls: 0,
                            release: held,
                        })),
                        frame_route: Some(rds_core::UniHello::DesktopFrames { id: session }),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            }
        });
        let conn = client.connect(server.addr(), rds_core::ALPN).await.unwrap();
        let mut desktop = DesktopSession::connect_opts(
            &conn,
            DesktopHello {
                display: 0,
                max_fps: 30,
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
        let tap = desktop.encoded.as_mut().unwrap();
        let first = tokio::time::timeout(Duration::from_secs(2), tap.recv())
            .await
            .unwrap()
            .unwrap();
        let second = tokio::time::timeout(Duration::from_secs(2), tap.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (first.header.seq, second.header.seq),
            (0, 1),
            "codec skip must not create a reference gap"
        );
        assert!(first.header.keyframe);
        assert!(
            !second.header.keyframe,
            "harmless skip caused an unnecessary IDR"
        );
        release.send(()).unwrap();
        serving.await.unwrap();
        drop(desktop);
        conn.close(0u32.into(), b"test complete");
        client.close().await;
        server.close().await;
    })
    .await
    .unwrap();
}

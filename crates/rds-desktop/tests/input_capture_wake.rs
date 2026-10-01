//! Accepted input must wake a capture source whose damage stream is idle.
use bytes::Bytes;
use rds_core::{
    Codec, DesktopControl, DesktopHello, FrameHeader, HelloAck, InputEvent, InputKind, StreamHello,
};
use rds_desktop::client::{DesktopSession, SessionOpts};
use rds_desktop::{
    DesktopError, FrameProducer, InputSink, Produced, ProducerControls, SessionClock,
    SessionConfig, serve_desktop_with,
};
use rds_net::{EndpointConfig, read_frame, write_frame};
use std::sync::atomic::Ordering;
use std::time::Duration;

struct IdleSource {
    idle: Option<tokio::sync::oneshot::Sender<()>>,
    stop: std::sync::mpsc::Receiver<()>,
    pause: Duration,
}
impl FrameProducer for IdleSource {
    fn produce(
        &mut self,
        seq: u64,
        controls: &ProducerControls,
        clock: &SessionClock,
    ) -> Option<Produced> {
        if seq > 1 {
            let _ = self.stop.recv_timeout(Duration::from_secs(3));
            return None;
        }
        if seq > 0 {
            if let Some(idle) = self.idle.take() {
                let _ = idle.send(());
            }
            std::thread::sleep(self.pause);
            loop {
                if self.stop.try_recv().is_ok() {
                    return None;
                }
                if controls.input_refresh_active(clock.now_ms()) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Some(Produced {
            header: FrameHeader {
                seq,
                capture_ts_ms: clock.now_ms(),
                encode_done_ts_ms: clock.now_ms(),
                send_ts_ms: 0,
                keyframe: controls.idr.swap(false, Ordering::Relaxed),
                codec: Codec::H264,
                width: 64,
                height: 64,
            },
            payload: Bytes::from_static(b"encoded"),
        })
    }
}
struct Input;
impl InputSink for Input {
    fn inject(&mut self, _: &InputEvent) -> Result<(), DesktopError> {
        Ok(())
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_input_wakes_idle_capture_without_requesting_a_keyframe() {
    for (view_only, pause) in [
        (false, Duration::ZERO),
        (true, Duration::ZERO),
        (false, Duration::from_millis(600)),
    ] {
        let config = EndpointConfig {
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        };
        let server = rds_net::bind_endpoint(config.clone()).await.unwrap();
        let client = rds_net::bind_endpoint(config).await.unwrap();
        let (stop, stopped) = std::sync::mpsc::channel();
        let (idle, idling) = tokio::sync::oneshot::channel();
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
                let _ = serve_desktop_with(
                    conn,
                    send,
                    recv,
                    hello,
                    SessionConfig {
                        view_only,
                        input_sink: Some(Box::new(Input)),
                        producer: Some(Box::new(IdleSource {
                            idle: Some(idle),
                            stop: stopped,
                            pause,
                        })),
                        frame_route: Some(rds_core::UniHello::DesktopFrames { id: session }),
                        ..Default::default()
                    },
                )
                .await;
            }
        });
        let conn = client.connect(server.addr(), rds_core::ALPN).await.unwrap();
        let mut desktop = DesktopSession::connect_opts(
            &conn,
            DesktopHello {
                display: 0,
                max_fps: 30,
                codec: Codec::H264,
                input_acks: true,
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
            desktop.encoded.as_mut().unwrap().recv(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(first.header.seq, 0);
        assert!(first.header.keyframe);
        tokio::time::timeout(Duration::from_secs(2), idling)
            .await
            .unwrap()
            .unwrap();
        desktop
            .control_sender()
            .send(DesktopControl::Input(InputEvent {
                seq: 7,
                event_ts_ms: 0,
                display_id: 0,
                kind: InputKind::KeyDown { code: 30 },
            }))
            .await
            .unwrap();
        let response = tokio::time::timeout(
            // Preserve the original 300 ms wake bound after the deliberately
            // paused producer resumes, rather than treating its pause as RTT.
            pause + Duration::from_millis(300),
            desktop.encoded.as_mut().unwrap().recv(),
        )
        .await;
        stop.send(()).unwrap();
        drop(desktop);
        conn.close(0u32.into(), b"fixture complete");
        tokio::time::timeout(Duration::from_secs(2), serving)
            .await
            .unwrap()
            .unwrap();
        server.close().await;
        client.close().await;
        if view_only {
            assert!(response.is_err(), "view-only input must not wake capture");
        } else {
            let frame = response
                .expect("accepted input remained in idle capture")
                .unwrap();
            assert_eq!(frame.header.seq, 1);
            assert!(
                !frame.header.keyframe,
                "capture wake must not request an IDR"
            );
        }
    }
}

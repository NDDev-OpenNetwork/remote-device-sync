//! Explicit attended capture qualification; never types or moves the pointer.
#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use rds_desktop::{
        Decoder, DesktopSource, EncodedFrame, H264Decoder, ProducerControls, SessionClock,
        WaylandDesktop,
    };
    use std::sync::Arc;
    use std::time::Duration;
    tracing_subscriber::fmt()
        .with_env_filter("rds_desktop::portal=info")
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
    let state = std::env::args_os()
        .nth(1)
        .ok_or("usage: wayland_check ABSOLUTE_PRIVATE_STATE_FILE")?;
    let verify_sync = match std::env::args().nth(2).as_deref() {
        None => false,
        Some("--verify-input-sync") => true,
        Some(_) => return Err("unknown qualification option".into()),
    };
    if std::env::args_os().nth(3).is_some() {
        return Err("too many qualification arguments".into());
    }
    eprintln!("Waiting for the local portal permission; select the monitors to share.");
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let cancelled = async {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    };
    let source =
        Arc::new(WaylandDesktop::open_with_cancel(std::path::Path::new(&state), cancelled).await?);
    let result = async {
        let caps = source.capabilities()?;
        for display in caps.displays {
            let backend = source.clone();
            tokio::task::spawn_blocking(move || -> Result<(), rds_desktop::DesktopError> {
                if verify_sync {
                    // This new source has never pressed a key. An unowned
                    // key-up is suppressed by the RDS hold aggregator; only
                    // the native EI sync round trip is sent to the compositor.
                    // This verifies lease/ACK plumbing without application input.
                    let mut sink = backend.input(display.index, (display.width, display.height))?;
                    sink.inject(&rds_core::InputEvent {
                        seq: 0,
                        event_ts_ms: 0,
                        display_id: display.index,
                        kind: rds_core::InputKind::KeyUp { code: 28 },
                    })?;
                    println!("display={} native_input_sync=acknowledged", display.index);
                }
                let mut producer = backend.producer(
                    display.index,
                    Duration::from_millis(33),
                    Some(1080),
                    Some((display.width, display.height)),
                )?;
                let controls = ProducerControls::new(4_000_000);
                let clock = SessionClock::default();
                let mut decoder = H264Decoder::new()?;
                let mut decoded = 0;
                for seq in 0..3 {
                    let frame = producer.produce(seq, &controls, &clock).ok_or_else(|| {
                        rds_desktop::DesktopError::Capture(
                            "native qualification produced no frame".into(),
                        )
                    })?;
                    if frame.payload.is_empty() {
                        continue;
                    }
                    if let Some(image) = decoder.decode(&EncodedFrame {
                        codec: frame.header.codec,
                        data: frame.payload,
                        keyframe: frame.header.keyframe,
                    })? {
                        if (image.width, image.height) != (frame.header.width, frame.header.height)
                        {
                            return Err(rds_desktop::DesktopError::Decode(
                                "native extent mismatch".into(),
                            ));
                        }
                        decoded += 1;
                    }
                }
                if decoded == 0 {
                    return Err(rds_desktop::DesktopError::Decode(
                        "no decoded native frames".into(),
                    ));
                }
                println!(
                    "display={} native={}x{} decoded_frames={decoded}",
                    display.index, display.width, display.height
                );
                Ok(())
            })
            .await??;
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    source.close().await;
    result
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Wayland serving requires Linux.");
}

//! Explicit attended capture qualification; never injects native input.
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
    eprintln!("Waiting for the local portal permission; select the monitors to share.");
    let source = Arc::new(WaylandDesktop::open(std::path::Path::new(&state)).await?);
    let result = async {
        let caps = source.capabilities()?;
        for display in caps.displays {
            let backend = source.clone();
            tokio::task::spawn_blocking(move || -> Result<(), rds_desktop::DesktopError> {
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

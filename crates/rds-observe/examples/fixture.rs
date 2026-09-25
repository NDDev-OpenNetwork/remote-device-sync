//! Synthetic input for the opt-in Vector/OpenObserve integration regression.
use rds_observe::{Config, Event, Format, Operation, Service};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let telemetry = rds_observe::install(Service::Bench, Config::new(Format::Json, "info")?)?;
    let result = telemetry
        .run(async {
            rds_observe::emit(Event::ListenerReady);
            let session =
                tracing::info_span!("rds.conn", session_id = 7u64, peer = "PRIVATE_SENTINEL");
            use tracing::Instrument as _;
            async {
                tracing::warn!(path = "PRIVATE_SENTINEL", "PRIVATE_SENTINEL");
                rds_observe::emit(Event::PeerRejected);
                let _: Result<(), ()> = rds_observe::observe(Operation::Connect, async {
                    tokio::time::sleep(std::time::Duration::from_millis(3)).await;
                    Err(())
                })
                .await;
                let _: Result<(), ()> =
                    rds_observe::observe(Operation::Connect, async { Ok(()) }).await;
            }
            .instrument(session)
            .await;
            Err::<(), ()>(()) // Exercise a failed process outcome with no raw cause.
        })
        .await;
    assert!(result.is_err());
    let shutdown = telemetry.shutdown();
    assert!(shutdown.drained);
    assert_eq!(shutdown.health.telemetry_dropped_total, 0);
    Ok(())
}

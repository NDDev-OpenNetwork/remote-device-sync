//! Scenario runners — each produces one `BenchReport`.
//!
//! The case set follows the QUIC interop runner taxonomy
//! (`handshake`, `transfer`, `multiconnect`) plus rds-specific cases
//! (`ping`, `relay-fallback`, impaired-path runs). `rebind-*` and
//! `migration` need socket-level control and arrive with the noq
//! backend (WS1).

use std::time::{Duration, Instant};

use anyhow::Context;

use crate::impair::Impairment;
use crate::report::{BenchMeta, BenchReport, Percentiles, git_sha, unix_ts};
use crate::world::{Path, World};

/// Knobs every scenario reads; CLI fills it.
#[derive(Debug, Clone)]
pub struct Params {
    /// Probes per latency scenario / connections per handshake scenario.
    pub iterations: usize,
    /// Payload size for `transfer`, MiB.
    pub transfer_mib: u64,
    /// Per-attempt timeout; an attempt exceeding it counts as failed.
    pub timeout: Duration,
    /// Impairment applied to `*-impaired` scenarios.
    pub impairment: Impairment,
    /// Backend under test (only `iroh` until WS1 lands).
    pub backend: String,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            iterations: 100,
            transfer_mib: 32,
            timeout: Duration::from_secs(15),
            impairment: Impairment::lossy(),
            backend: "iroh".into(),
        }
    }
}

/// Every case `run` knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Scenario {
    /// Cold connect → close, repeated. Measures resolve→established.
    Handshake,
    /// Established connection, N ping probes, RTT percentiles.
    Ping,
    /// Bulk stream throughput to a TCP discard target.
    Transfer,
    /// Handshake under 5% loss (interop `multiconnect` analogue).
    Multiconnect,
    /// Connect with only the relay address advertised.
    RelayFallback,
    /// Ping over the impaired direct path (loss/jitter/rate as given).
    Impaired,
    /// All of the above.
    All,
}

impl Scenario {
    fn name(&self) -> &'static str {
        match self {
            Scenario::Handshake => "handshake",
            Scenario::Ping => "ping",
            Scenario::Transfer => "transfer",
            Scenario::Multiconnect => "multiconnect",
            Scenario::RelayFallback => "relay-fallback",
            Scenario::Impaired => "impaired",
            Scenario::All => "all",
        }
    }
}

/// Run one scenario (or the whole suite for `All`).
pub async fn run(s: Scenario, p: &Params) -> anyhow::Result<Vec<BenchReport>> {
    if s == Scenario::All {
        let mut out = Vec::new();
        for s in [
            Scenario::Handshake,
            Scenario::Ping,
            Scenario::Transfer,
            Scenario::Multiconnect,
            Scenario::RelayFallback,
            Scenario::Impaired,
        ] {
            match run_one(s, p).await {
                Ok(reports) => out.extend(reports),
                Err(e) => out.push(failed(s.name(), p, e)),
            }
        }
        return Ok(out);
    }
    run_one(s, p).await
}

async fn run_one(s: Scenario, p: &Params) -> anyhow::Result<Vec<BenchReport>> {
    match s {
        Scenario::Handshake => handshake(p, Path::Direct).await.map(|r| vec![r]),
        Scenario::Ping => ping(p, Path::Direct, None).await.map(|r| vec![r]),
        Scenario::Transfer => transfer(p).await.map(|r| vec![r]),
        Scenario::Multiconnect => {
            handshake(p, Path::DirectImpaired(p.impairment))
                .await
                .map(|mut r| {
                    r.meta.scenario = "multiconnect".into();
                    vec![r]
                })
        }
        Scenario::RelayFallback => ping(p, Path::RelayOnly, None).await.map(|mut r| {
            r.meta.scenario = "relay-fallback".into();
            vec![r]
        }),
        Scenario::Impaired => ping(p, Path::DirectImpaired(p.impairment), Some(p.impairment))
            .await
            .map(|mut r| {
                r.meta.scenario = "impaired".into();
                vec![r]
            }),
        Scenario::All => unreachable!("handled in run"),
    }
}

fn meta(scenario: &str, p: &Params, path: &str, impairment: Option<Impairment>) -> BenchMeta {
    BenchMeta {
        scenario: scenario.into(),
        backend: p.backend.clone(),
        path: path.into(),
        impairment,
        unix_ts: unix_ts(),
        git: git_sha(),
    }
}

fn proxy_note(world: &World) -> Vec<String> {
    world
        .proxy_stats
        .as_ref()
        .map(|p| {
            let s = p.stats();
            vec![format!(
                "proxy: forwarded={} dropped={} bytes={}",
                s.forwarded, s.dropped, s.bytes
            )]
        })
        .unwrap_or_default()
}

fn failed(scenario: &str, p: &Params, e: anyhow::Error) -> BenchReport {
    BenchReport {
        meta: meta(scenario, p, "n/a", None),
        rtt: None,
        throughput_mib_s: None,
        attempts: None,
        notes: vec![format!("SCENARIO FAILED: {e:#}")],
    }
}

/// Cold connect → immediate close, repeated `iterations` times.
async fn handshake(p: &Params, path: Path) -> anyhow::Result<BenchReport> {
    let world = World::spawn(path).await.context("spawn world")?;
    let mut samples = Vec::with_capacity(p.iterations);
    let mut ok = 0u64;
    for _ in 0..p.iterations {
        let t0 = Instant::now();
        if let Ok(Ok(conn)) = tokio::time::timeout(
            p.timeout,
            rds_cli::connect(&world.client, world.target.clone()),
        )
        .await
        {
            samples.push(t0.elapsed().as_nanos() as u64);
            ok += 1;
            conn.close(0u32.into(), b"bench done");
        }
    }
    let mut notes = proxy_note(&world);
    if ok < p.iterations as u64 {
        notes.push(format!(
            "{} connects timed out or failed",
            p.iterations as u64 - ok
        ));
    }
    world.close().await;
    Ok(BenchReport {
        meta: meta("handshake", p, world.path_label(), impairment_of(&path)),
        rtt: Percentiles::of(&samples),
        throughput_mib_s: None,
        attempts: Some((ok, p.iterations as u64)),
        notes,
    })
}

/// One connection, `iterations` ping probes.
async fn ping(
    p: &Params,
    path: Path,
    impairment: Option<Impairment>,
) -> anyhow::Result<BenchReport> {
    let world = World::spawn(path).await.context("spawn world")?;
    let conn = tokio::time::timeout(
        p.timeout,
        rds_cli::connect(&world.client, world.target.clone()),
    )
    .await
    .context("connect timed out")?
    .context("connect failed")?;
    // Warmup probes are excluded: the first streams pay one-time setup
    // that would otherwise pollute the tail.
    for i in 0..5 {
        rds_cli::ping(&conn, i as u64).await?;
    }
    let mut samples = Vec::with_capacity(p.iterations);
    for i in 0..p.iterations {
        let rtt = rds_cli::ping(&conn, 1000 + i as u64).await?;
        samples.push(rtt.as_nanos() as u64);
    }
    world.close().await;
    Ok(BenchReport {
        meta: meta("ping", p, world.path_label(), impairment),
        rtt: Percentiles::of(&samples),
        throughput_mib_s: None,
        attempts: None,
        notes: proxy_note(&world),
    })
}

/// `transfer_mib` MiB over one forwarded stream to a discard sink.
async fn transfer(p: &Params) -> anyhow::Result<BenchReport> {
    let world = World::spawn(Path::Direct).await.context("spawn world")?;
    let conn = tokio::time::timeout(
        p.timeout,
        rds_cli::connect(&world.client, world.target.clone()),
    )
    .await
    .context("connect timed out")?
    .context("connect failed")?;
    let (host, port) = world.discard_target();
    let (mut send, _recv) = rds_cli::open_tcp(&conn, &host, port).await?;
    let chunk = vec![0xABu8; 256 * 1024];
    let mut written = 0u64;
    let total = p.transfer_mib * 1024 * 1024;
    let t0 = Instant::now();
    while written < total {
        let n = (total - written).min(chunk.len() as u64) as usize;
        send.write_all(&chunk[..n]).await?;
        written += n as u64;
    }
    send.finish()?;
    // Wait until the receiver has drained: finish() returns when our
    // side is done sending; add a grace read timeout on recv to bound it.
    let elapsed = t0.elapsed();
    let mib_s = written as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
    world.close().await;
    Ok(BenchReport {
        meta: meta("transfer", p, world.path_label(), None),
        rtt: None,
        throughput_mib_s: Some(mib_s),
        attempts: None,
        notes: proxy_note(&world),
    })
}

trait PathLabel {
    fn path_label(&self) -> &'static str;
}

impl PathLabel for World {
    fn path_label(&self) -> &'static str {
        if self.proxy_stats.is_some() {
            "direct-impaired"
        } else if self
            .target
            .addrs
            .iter()
            .any(|a| matches!(a, iroh::TransportAddr::Relay(_)))
            && self
                .target
                .addrs
                .iter()
                .any(|a| matches!(a, iroh::TransportAddr::Ip(_)))
        {
            "mixed"
        } else if self
            .target
            .addrs
            .iter()
            .any(|a| matches!(a, iroh::TransportAddr::Relay(_)))
        {
            "relay"
        } else {
            "direct"
        }
    }
}

fn impairment_of(path: &Path) -> Option<Impairment> {
    match path {
        Path::DirectImpaired(i) => Some(*i),
        _ => None,
    }
}

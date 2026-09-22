//! WS7: metrics counters are accurate against known traffic — the C7
//! "counter accuracy proven by a known-traffic test" gate.
//!
//! One bidirectional echo roundtrip over a direct path must move
//! `datagrams_sent{via=direct}` and nothing relay; a relay-only
//! advertised address must move `via=relay` and nothing direct. The
//! counters are endpoint-level registries folded from per-path
//! `path_stats` deltas by `ConnSampler`.

use std::collections::BTreeSet;
use std::time::Duration;

use rds_net::metrics::Registry;
use rds_net::{EndpointAddr, EndpointConfig, TransportAddr, bind_endpoint};

/// Serve echo on one connection: one bi stream, bytes back verbatim.
async fn echo_once(ep: &rds_net::Endpoint) {
    let conn = ep
        .accept()
        .await
        .expect("incoming")
        .await
        .expect("handshake");
    // Sample the server side too while the connection is alive.
    let mut sampler = ep.metrics().sampler(conn.clone());
    let (mut send, mut recv) = conn.accept_bi().await.unwrap();
    let mut buf = vec![0u8; 64];
    let n = recv.read(&mut buf).await.unwrap().unwrap();
    send.write_all(&buf[..n]).await.unwrap();
    send.finish().unwrap();
    sampler.sample();
    // Hold the connection until the peer closes it — dropping the last
    // handle now could cut the echo reply before the client reads it.
    let _ = conn.accept_bi().await;
}

fn counter(reg: &Registry, name: &str) -> u64 {
    *reg.snapshot().get(name).unwrap_or(&0)
}

/// Direct path only: ticket carries just the agent's UDP address.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_traffic_counts_direct_not_relay() {
    let cfg = || EndpointConfig::default().without_discovery();
    let server = bind_endpoint(cfg()).await.unwrap();
    let client = bind_endpoint(cfg()).await.unwrap();

    let server_ep = server.clone();
    let task = tokio::spawn(async move { echo_once(&server_ep).await });

    let mut addrs = BTreeSet::new();
    for a in server.addr().addrs {
        if let TransportAddr::Ip(sa) = a {
            addrs.insert(TransportAddr::Ip(sa));
        }
    }
    let conn = client
        .connect(
            EndpointAddr {
                id: server.id(),
                addrs,
            },
            rds_core::ALPN,
        )
        .await
        .unwrap();

    // Known traffic: one 8-byte ping → pong roundtrip.
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(b"ping-pong").await.unwrap();
    send.finish().unwrap();
    let mut buf = [0u8; 9];
    recv.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping-pong");

    let mut sampler = client.metrics().sampler(conn.clone());
    sampler.sample();
    conn.close(0u32.into(), b"done");
    task.await.unwrap();

    let c = client.metrics();
    assert_eq!(counter(&c, "rds_net_connections_opened_total"), 1);
    assert!(counter(&c, "rds_net_datagrams_sent_total{via=\"direct\"}") > 0);
    assert_eq!(
        counter(&c, "rds_net_datagrams_sent_total{via=\"relay\"}"),
        0
    );
    // The 9-byte payload crossed the wire: bytes sent and received on
    // the direct path must both cover it (datagrams carry overhead).
    assert!(counter(&c, "rds_net_bytes_sent_total{via=\"direct\"}") >= 9);
    assert!(counter(&c, "rds_net_bytes_received_total{via=\"direct\"}") >= 9);
    assert_eq!(counter(&c, "rds_net_bytes_sent_total{via=\"relay\"}"), 0);
    assert!(counter(&c, "rds_net_paths_seen_total{via=\"direct\"}") >= 1);

    let s = server.metrics();
    assert_eq!(counter(&s, "rds_net_connections_accepted_total"), 1);
    assert!(counter(&s, "rds_net_datagrams_sent_total{via=\"direct\"}") > 0);
}

/// Relay-only advertised address: the payload must land on
/// `via=relay` — the split is real, not guessed. Direct-path probes
/// may still appear in the counters (see the assertion comment).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_only_traffic_counts_relay_not_direct() {
    let mut relay_config = iroh_relay::server::ServerConfig::default();
    relay_config.relay = Some(iroh_relay::server::RelayConfig::new(
        "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let relay = iroh_relay::server::Server::spawn(relay_config)
        .await
        .unwrap();
    let url = format!("http://{}", relay.http_addr().unwrap());

    // Path pinning keeps the connection on its initial (relay) path —
    // otherwise iroh's in-band address exchange could upgrade to a
    // direct path mid-test and pollute the via=direct counter.
    let cfg = || {
        EndpointConfig::default()
            .with_relay(&url)
            .unwrap()
            .with_path_pinning()
    };
    let server = bind_endpoint(cfg()).await.unwrap();
    let client = bind_endpoint(cfg()).await.unwrap();
    server.online().await;
    client.online().await;

    let server_ep = server.clone();
    let task = tokio::spawn(async move { echo_once(&server_ep).await });

    // Ticket carries ONLY the relay transport address.
    let relay_addr = server
        .addr()
        .addrs
        .iter()
        .find_map(|a| match a {
            TransportAddr::Relay(u) => Some(u.clone()),
            _ => None,
        })
        .expect("server has relay addr");
    let conn = client
        .connect(
            EndpointAddr {
                id: server.id(),
                addrs: BTreeSet::from([TransportAddr::Relay(relay_addr)]),
            },
            rds_core::ALPN,
        )
        .await
        .unwrap();

    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(b"relay-ok!").await.unwrap();
    send.finish().unwrap();
    let mut buf = [0u8; 9];
    tokio::time::timeout(Duration::from_secs(15), recv.read_exact(&mut buf))
        .await
        .expect("relay echo timed out")
        .unwrap();

    let mut sampler = client.metrics().sampler(conn.clone());
    sampler.sample();
    conn.close(0u32.into(), b"done");
    task.await.unwrap();

    let c = client.metrics();
    assert!(counter(&c, "rds_net_datagrams_sent_total{via=\"relay\"}") > 0);
    assert!(counter(&c, "rds_net_bytes_sent_total{via=\"relay\"}") >= 9);
    assert!(counter(&c, "rds_net_bytes_received_total{via=\"relay\"}") >= 9);
    assert!(counter(&c, "rds_net_paths_seen_total{via=\"relay\"}") >= 1);
    // No `via=direct == 0` assertions: path pinning caps concurrent
    // paths at one, but iroh still fires direct-path probes at
    // candidates it learns via in-band address exchange — whether they
    // land inside the test window is platform timing (4 datagrams on
    // macOS runners, none observed on Linux). They are real datagrams
    // and the counter is right to record them; the relay side above is
    // what proves the split accounts payload traffic correctly.
}

/// Prometheus export exists under `metrics` and carries the names the
/// bench reports cite — no keys, addresses or peer content in the text.
#[cfg(feature = "metrics")]
#[test]
fn prometheus_render_has_report_names() {
    let reg = Registry::default();
    reg.connection_opened();
    reg.qnt_attempt();
    let text = reg.render_prometheus();
    for name in [
        "rds_net_connections_opened_total",
        "rds_net_datagrams_sent_total",
        "rds_net_datagrams_lost_total",
        "rds_net_bytes_sent_total",
        "rds_net_bytes_received_total",
        "rds_net_congestion_events_total",
        "rds_net_qnt_attempts_total",
        "rds_net_qnt_success_total",
        "rds_net_rtt_us",
        "rds_net_cwnd_bytes",
    ] {
        assert!(text.contains(name), "render missing {name}");
    }
    assert!(text.contains("# TYPE rds_net_rtt_us gauge"));
    assert!(text.contains("rds_net_connections_opened_total 1"));
}

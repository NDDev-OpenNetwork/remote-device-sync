//! Test world: an in-process relay, a serving agent and a client
//! endpoint, with the advertised address controlling which path QUIC
//! actually uses.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;

use rds_agent::{Agent, AgentPolicy};
use rds_net::{Endpoint, EndpointAddr, EndpointConfig, TransportAddr, bind_endpoint};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::impair::{self, Impairment, Proxy};

/// Which path the advertised address exposes.
#[derive(Debug, Clone, Copy)]
pub enum Path {
    /// Ticket carries the agent's UDP socket address only.
    Direct,
    /// Ticket carries a UDP proxy address that impairs the direct path.
    DirectImpaired(Impairment),
    /// Ticket carries only the relay URL — QUIC must use the relay path.
    RelayOnly,
    /// Direct plus relay, as a real deployment would advertise.
    Mixed,
}

impl Path {
    pub fn label(&self) -> &'static str {
        match self {
            Path::Direct => "direct",
            Path::DirectImpaired(_) => "direct-impaired",
            Path::RelayOnly => "relay",
            Path::Mixed => "mixed",
        }
    }
}

/// Everything a scenario needs; dropping `tasks` stops the world.
pub struct World {
    pub client: Endpoint,
    pub target: EndpointAddr,
    pub agent: Arc<Agent>,
    pub discard_port: u16,
    pub proxy_stats: Option<Arc<Proxy>>,
    tasks: Vec<JoinHandle<()>>,
    /// Keeps the in-process relay alive for the world's lifetime.
    _relay: Option<iroh_relay::server::Server>,
}

impl World {
    /// TCP discard service port the agent permits (for `transfer`).
    pub fn discard_target(&self) -> (String, u16) {
        ("127.0.0.1".into(), self.discard_port)
    }

    /// Graceful endpoint shutdown; scenarios call this before the
    /// report so drop-aborts don't pollute logs with ungraceful-close
    /// errors.
    pub async fn close(&self) {
        self.client.close().await;
        self.agent.endpoint.close().await;
    }

    /// Metrics snapshot for the report (G7): if `conn` is given, its
    /// cumulative path stats are folded into the client registry first,
    /// then both registries are read — `client_*`/`agent_*` prefixes.
    /// The agent samples its own connections once a second, so a short
    /// scenario still sees its side via this one-shot fold.
    pub fn metrics_snapshot(
        &self,
        conn: Option<&rds_net::Connection>,
    ) -> std::collections::BTreeMap<String, u64> {
        let mut out = std::collections::BTreeMap::new();
        // Keep the observation owner through the scrape: dropping a sampler
        // invalidates its last selected-path gauge, without retaining I/O.
        let mut sampler = conn.map(|conn| self.client.metrics().sampler(conn.clone()));
        if let Some(sampler) = &mut sampler {
            sampler.sample();
        }
        for (k, v) in self.client.metrics().snapshot() {
            out.insert(format!("client_{k}"), v);
        }
        for (k, v) in self.agent.endpoint.metrics().snapshot() {
            out.insert(format!("agent_{k}"), v);
        }
        out
    }
}

impl Drop for World {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl World {
    /// Spin up relay + agent + client on `backend` and return the world
    /// plus the target address the client should dial for `path`.
    ///
    /// The owned `noq` backend has no relay transport yet (WS2): relay
    /// paths on it fail here with a clear error, and no relay process
    /// is spawned.
    pub async fn spawn(path: Path, backend: rds_net::Backend) -> anyhow::Result<World> {
        let mut tasks = Vec::new();

        let wants_relay = matches!(path, Path::RelayOnly | Path::Mixed);
        if wants_relay && backend != rds_net::Backend::Iroh {
            anyhow::bail!(
                "path {:?} needs a relay; backend {backend:?} has no relay transport yet (WS2)",
                path.label()
            );
        }

        // Relay is spawned for the iroh backend (Minimal preset, no
        // third-party lookups); whether traffic uses it is decided by
        // the advertised ticket.
        let relay = match backend {
            rds_net::Backend::Iroh => {
                let mut config = iroh_relay::server::ServerConfig::default();
                config.relay = Some(iroh_relay::server::RelayConfig::new(
                    "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
                ));
                Some(iroh_relay::server::Server::spawn(config).await?)
            }
            #[allow(unreachable_patterns)]
            _ => None,
        };
        let relay_url = relay
            .as_ref()
            .map(|r| format!("http://{}", r.http_addr().unwrap()));

        let endpoint_config = |relay_url: &Option<String>| -> anyhow::Result<EndpointConfig> {
            let mut config = EndpointConfig::default().with_backend(backend);
            if let Some(url) = relay_url {
                config = config.with_relay(url)?;
            }
            Ok(config)
        };
        let agent_ep = bind_endpoint(endpoint_config(&relay_url)?).await?;
        let client_ep = bind_endpoint(endpoint_config(&relay_url)?).await?;
        agent_ep.online().await;
        client_ep.online().await;

        // TCP discard sink for throughput scenarios.
        let discard_port = spawn_discard(&mut tasks).await;

        let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), discard_port));
        policy.allow.insert(client_ep.id());
        policy.allow_any_tcp = true; // bench targets are ours
        let agent = Arc::new(Agent::new(agent_ep, policy));
        tasks.push(tokio::spawn({
            let agent = agent.clone();
            async move {
                let _ = agent.run().await;
            }
        }));

        // The agent's real UDP address and relay address from its addr().
        let advertised = agent.endpoint.addr();
        let mut ip_addrs = advertised.addrs.iter().filter_map(|a| match a {
            TransportAddr::Ip(sa) => Some(*sa),
            _ => None,
        });
        let agent_udp = ip_addrs
            .find(|sa| sa.ip().is_loopback())
            .or_else(|| {
                advertised.addrs.iter().find_map(|a| match a {
                    TransportAddr::Ip(sa) => Some(*sa),
                    _ => None,
                })
            })
            .ok_or_else(|| anyhow::anyhow!("agent endpoint advertises no udp addr"))?;
        let agent_relay = advertised.addrs.iter().find_map(|a| match a {
            TransportAddr::Relay(u) => Some(u.clone()),
            _ => None,
        });

        let mut proxy_stats = None;
        let mut addrs = BTreeSet::new();
        match path {
            Path::Direct => {
                addrs.insert(TransportAddr::Ip(agent_udp));
            }
            Path::DirectImpaired(cfg) => {
                let proxy = impair::spawn(agent_udp, cfg).await?;
                addrs.insert(TransportAddr::Ip(proxy.listen));
                proxy_stats = Some(Arc::new(proxy));
            }
            Path::RelayOnly => {
                let relay_addr = agent_relay
                    .ok_or_else(|| anyhow::anyhow!("agent endpoint has no relay addr"))?;
                addrs.insert(TransportAddr::Relay(relay_addr));
            }
            Path::Mixed => {
                let relay_addr = agent_relay
                    .ok_or_else(|| anyhow::anyhow!("agent endpoint has no relay addr"))?;
                addrs.insert(TransportAddr::Ip(agent_udp));
                addrs.insert(TransportAddr::Relay(relay_addr));
            }
        }

        Ok(World {
            client: client_ep,
            target: EndpointAddr {
                id: agent.id(),
                addrs,
            },
            agent,
            discard_port,
            proxy_stats,
            tasks,
            _relay: relay,
        })
    }
}

/// TCP server that reads and discards — the `transfer` scenario's target.
async fn spawn_discard(tasks: &mut Vec<JoinHandle<()>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tasks.push(tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                while sock.read(&mut buf).await.unwrap_or(0) > 0 {}
            });
        }
    }));
    port
}

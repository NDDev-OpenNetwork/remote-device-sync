//! The agent: accepts authenticated rds connections and serves streams.
//!
//! Authorization is an allowlist of peer `EndpointId`s checked as soon as
//! the QUIC handshake completes — before any service stream is read. TCP
//! forwarding is restricted to an explicit set of `(host, port)` targets;
//! the default set is exactly the configured SSH socket.

use std::collections::HashSet;
use std::sync::Arc;

use rds_core::{
    AgentInfo, HelloAck, PROTOCOL_VERSION, ServiceKind, StreamHello, read_frame, write_frame,
};
use rds_net::{Connection, Endpoint, EndpointId};
use tokio::net::TcpStream;
use tracing::{debug, info, warn};

/// Runtime policy for the agent.
#[derive(Debug, Clone)]
pub struct AgentPolicy {
    /// Peers allowed to open any stream at all.
    pub allow: HashSet<EndpointId>,
    /// TCP targets the `TcpConnect` service may splice to.
    /// The default is the configured SSH socket only.
    pub tcp_targets: HashSet<(String, u16)>,
    /// Accept any TCP target. Development escape hatch.
    pub allow_any_tcp: bool,
}

impl AgentPolicy {
    pub fn ssh_only(ssh: (String, u16)) -> Self {
        Self {
            allow: HashSet::new(),
            tcp_targets: HashSet::from([ssh]),
            allow_any_tcp: false,
        }
    }

    pub fn permits_tcp(&self, host: &str, port: u16) -> bool {
        self.allow_any_tcp
            || self
                .tcp_targets
                .iter()
                .any(|(h, p)| *p == port && h.eq_ignore_ascii_case(host))
    }
}

/// A bound agent: endpoint plus policy, ready to `run`.
pub struct Agent {
    pub endpoint: Endpoint,
    pub policy: Arc<AgentPolicy>,
    desktop: bool,
}

impl Agent {
    pub fn new(endpoint: Endpoint, policy: AgentPolicy) -> Self {
        Self {
            endpoint,
            policy: Arc::new(policy),
            desktop: cfg!(feature = "desktop"),
        }
    }

    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// Accept connections until the endpoint closes.
    pub async fn run(&self) -> anyhow::Result<()> {
        info!(id = %self.endpoint.id(), "agent listening");
        while let Some(incoming) = self.endpoint.accept().await {
            let policy = self.policy.clone();
            let desktop = self.desktop;
            tokio::spawn(async move {
                match incoming.await {
                    Ok(conn) => {
                        if let Err(e) = serve_connection(conn, policy, desktop).await {
                            debug!("connection ended: {e}");
                        }
                    }
                    Err(e) => debug!("incoming handshake failed: {e}"),
                }
            });
        }
        Ok(())
    }

    /// Serve a single already-established connection.
    pub async fn serve(&self, conn: Connection) -> anyhow::Result<()> {
        serve_connection(conn, self.policy.clone(), self.desktop).await
    }
}

async fn serve_connection(
    conn: Connection,
    policy: Arc<AgentPolicy>,
    desktop: bool,
) -> anyhow::Result<()> {
    let peer = conn.remote_id();
    if !policy.allow.contains(&peer) {
        warn!(%peer, "rejected: endpoint id not in allowlist");
        conn.close(1u32.into(), b"not allowed");
        anyhow::bail!("peer {peer} not in allowlist");
    }
    info!(%peer, "peer connected");
    loop {
        let (send, recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(e) => {
                debug!(%peer, "connection closed: {e}");
                return Ok(());
            }
        };
        let policy = policy.clone();
        let conn = conn.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_stream(conn, send, recv, policy, desktop).await {
                debug!("stream ended: {e}");
            }
        });
    }
}

async fn serve_stream(
    conn: Connection,
    mut send: rds_net::SendStream,
    mut recv: rds_net::RecvStream,
    policy: Arc<AgentPolicy>,
    desktop: bool,
) -> anyhow::Result<()> {
    let _ = &conn;
    let hello: StreamHello = read_frame(&mut recv).await?;
    match hello {
        StreamHello::Ping { nonce } => {
            write_frame(&mut send, &HelloAck::Ok).await?;
            send.write_all(&nonce.to_be_bytes()).await?;
            send.finish()?;
        }
        StreamHello::Info => {
            let info = AgentInfo {
                protocol: PROTOCOL_VERSION,
                version: env!("CARGO_PKG_VERSION").to_string(),
                hostname: hostname(),
                services: {
                    let mut s = vec![ServiceKind::Ping, ServiceKind::Info, ServiceKind::Tcp];
                    if desktop {
                        s.push(ServiceKind::Desktop);
                    }
                    s
                },
                desktop: desktop_caps(desktop),
            };
            write_frame(&mut send, &HelloAck::Info(info)).await?;
            send.finish()?;
        }
        StreamHello::TcpConnect { host, port } => {
            if !policy.permits_tcp(&host, port) {
                write_frame(
                    &mut send,
                    &HelloAck::Error {
                        message: format!("tcp target {host}:{port} not permitted"),
                    },
                )
                .await?;
                anyhow::bail!("tcp target {host}:{port} rejected");
            }
            match TcpStream::connect((host.as_str(), port)).await {
                Ok(mut tcp) => {
                    write_frame(&mut send, &HelloAck::Ok).await?;
                    let mut quic = tokio::io::join(recv, send);
                    tokio::io::copy_bidirectional(&mut tcp, &mut quic).await?;
                }
                Err(e) => {
                    write_frame(
                        &mut send,
                        &HelloAck::Error {
                            message: format!("connect {host}:{port} failed: {e}"),
                        },
                    )
                    .await?;
                }
            }
        }
        StreamHello::Desktop(hello) => {
            if desktop {
                #[cfg(feature = "desktop")]
                match rds_desktop::capabilities() {
                    Ok(caps) => {
                        write_frame(&mut send, &HelloAck::Desktop(caps)).await?;
                        rds_desktop::serve_desktop(conn, send, recv, hello).await?;
                    }
                    Err(e) => {
                        write_frame(
                            &mut send,
                            &HelloAck::Error {
                                message: format!("desktop unavailable: {e}"),
                            },
                        )
                        .await?;
                    }
                }
                #[cfg(not(feature = "desktop"))]
                unreachable!()
            } else {
                let _ = hello;
                write_frame(
                    &mut send,
                    &HelloAck::Error {
                        message: "agent built without desktop support".into(),
                    },
                )
                .await?;
            }
        }
    }
    Ok(())
}

fn desktop_caps(enabled: bool) -> Option<rds_core::DesktopCaps> {
    #[cfg(feature = "desktop")]
    if enabled {
        return rds_desktop::capabilities().ok();
    }
    let _ = enabled;
    None
}

fn hostname() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
}

//! Shared relay configuration, identity initialization and runtime composition.
use clap::{Args, ValueEnum};
use iroh::EndpointId;
use iroh_relay::server::TlsConfig;
use std::{net::SocketAddr, num::NonZeroU16, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum RelayBackend {
    #[default]
    Iroh,
    Noq,
}

#[derive(Args, Default)]
pub struct RelayArgs {
    /// Relay implementation. The noq mode requires the owned-relay build feature.
    #[arg(long, value_enum, default_value = "iroh")]
    pub relay_backend: RelayBackend,
    /// Persistent identity for the owned relay, separate from endpoint identities.
    #[arg(long)]
    pub relay_key_file: Option<PathBuf>,
    /// Owned relay connections, including incomplete handshakes and registrations.
    #[arg(long)]
    pub relay_max_connections: Option<NonZeroU16>,
    /// Explicitly allow unknown peers on an owned relay for isolated development.
    #[arg(long, conflicts_with = "allow")]
    pub development_open_relay: bool,
    /// Allowed endpoint id (repeatable). Owned production mode requires entries.
    #[arg(long = "allow")]
    pub allow: Vec<EndpointId>,
    /// PEM certificate chain for the iroh HTTPS relay.
    #[arg(long, requires = "tls_key", conflicts_with = "tls_acme_domain")]
    pub tls_cert: Option<PathBuf>,
    #[arg(long, requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,
    /// Iroh HTTPS bind address, default 0.0.0.0:3443 when TLS is enabled.
    #[arg(long)]
    pub tls_https_addr: Option<SocketAddr>,
    /// Iroh in-process ACME domain (repeatable).
    #[arg(long, conflicts_with = "tls_cert")]
    pub tls_acme_domain: Vec<String>,
    #[arg(long, requires = "tls_acme_domain")]
    pub tls_acme_contact: Vec<String>,
    #[arg(long, requires = "tls_acme_domain")]
    pub tls_acme_cache: Option<PathBuf>,
    #[arg(long, requires = "tls_acme_domain")]
    pub tls_acme_staging: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RelayConfigError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("relay TLS configuration: {0}")]
    Tls(#[source] anyhow::Error),
}

pub struct PreparedRelay {
    backend: PreparedBackend,
}
enum PreparedBackend {
    Iroh {
        allow: Vec<EndpointId>,
        tls: Option<Box<TlsConfig>>,
    },
    #[cfg(feature = "owned-relay")]
    Noq {
        allow: Vec<EndpointId>,
        key_file: PathBuf,
        limits: crate::server::ServerLimits,
    },
}

impl RelayArgs {
    /// Validate and read TLS inputs, without creating identity/catalog state,
    /// binding sockets, contacting ACME, or mutating certificate caches.
    pub fn prepare(self) -> Result<PreparedRelay, RelayConfigError> {
        use RelayConfigError::Invalid;
        match self.relay_backend {
            RelayBackend::Iroh => {
                if self.relay_key_file.is_some()
                    || self.relay_max_connections.is_some()
                    || self.development_open_relay
                {
                    return Err(Invalid("owned-relay flags require --relay-backend noq"));
                }
                let manual = self.tls_cert.is_some() || self.tls_key.is_some();
                let acme = !self.tls_acme_domain.is_empty();
                if manual && acme {
                    return Err(Invalid("manual relay TLS and ACME are mutually exclusive"));
                }
                if !acme
                    && (!self.tls_acme_contact.is_empty()
                        || self.tls_acme_cache.is_some()
                        || self.tls_acme_staging)
                {
                    return Err(Invalid("ACME options require --tls-acme-domain"));
                }
                if !manual && !acme && self.tls_https_addr.is_some() {
                    return Err(Invalid(
                        "--tls-https-addr requires manual relay TLS or ACME",
                    ));
                }
                let address = self
                    .tls_https_addr
                    .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 3443)));
                let tls = crate::tls_from_flags(
                    address,
                    self.tls_cert,
                    self.tls_key,
                    self.tls_acme_domain,
                    self.tls_acme_contact,
                    self.tls_acme_cache,
                    self.tls_acme_staging,
                )
                .and_then(|tls| tls.map(crate::tls_config).transpose())
                .map_err(RelayConfigError::Tls)?
                .map(Box::new);
                Ok(PreparedRelay {
                    backend: PreparedBackend::Iroh {
                        allow: self.allow,
                        tls,
                    },
                })
            }
            RelayBackend::Noq => {
                #[cfg(not(feature = "owned-relay"))]
                {
                    Err(Invalid(
                        "owned relay backend unavailable: build with the owned-relay feature",
                    ))
                }
                #[cfg(feature = "owned-relay")]
                {
                    if self.tls_cert.is_some()
                        || self.tls_key.is_some()
                        || self.tls_https_addr.is_some()
                        || !self.tls_acme_domain.is_empty()
                        || !self.tls_acme_contact.is_empty()
                        || self.tls_acme_cache.is_some()
                        || self.tls_acme_staging
                    {
                        return Err(Invalid(
                            "HTTP relay TLS/ACME flags do not apply to the owned QUIC relay",
                        ));
                    }
                    if self.development_open_relay && !self.allow.is_empty() {
                        return Err(Invalid(
                            "development-open relay mode conflicts with an allowlist",
                        ));
                    }
                    if !self.development_open_relay && self.allow.is_empty() {
                        return Err(Invalid(
                            "owned relay requires --allow entries or explicit --development-open-relay",
                        ));
                    }
                    let key_file = self
                        .relay_key_file
                        .ok_or(Invalid("owned relay requires --relay-key-file"))?;
                    let Some(std::path::Component::Normal(name)) =
                        key_file.components().next_back()
                    else {
                        return Err(Invalid("relay key path must name a file"));
                    };
                    if name
                        .as_encoded_bytes()
                        .get(..9)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b".rds-key-"))
                    {
                        return Err(Invalid(
                            "relay key path uses the reserved identity transaction namespace",
                        ));
                    }

                    let limits = crate::server::ServerLimits {
                        max_connections: self
                            .relay_max_connections
                            .unwrap_or(crate::server::ServerLimits::default().max_connections),
                    };
                    Ok(PreparedRelay {
                        backend: PreparedBackend::Noq {
                            allow: self.allow,
                            key_file,
                            limits,
                        },
                    })
                }
            }
        }
    }
}

/// Startup/shutdown failures remain distinguishable from flag validation.
#[derive(Debug, thiserror::Error)]
pub enum RelayRuntimeError {
    #[cfg(feature = "owned-relay")]
    #[error("relay identity storage: {0}")]
    Identity(#[from] rds_net::KeyStoreError),
    #[cfg(feature = "owned-relay")]
    #[error("relay identity worker: {0}")]
    IdentityTask(#[from] tokio::task::JoinError),
    #[error("relay startup: {0}")]
    Startup(#[source] anyhow::Error),
    #[error("iroh relay shutdown: {0}")]
    IrohShutdown(#[source] anyhow::Error),
    #[cfg(feature = "owned-relay")]
    #[error(transparent)]
    OwnedShutdown(#[from] crate::server::ShutdownError),
}

/// Validated inputs and initialized identity, without any listening socket.
/// It can be prepared before opening the host's durable directory state.
pub struct ReadyRelay {
    backend: ReadyBackend,
}
enum ReadyBackend {
    Iroh {
        allow: Vec<EndpointId>,
        tls: Option<Box<TlsConfig>>,
    },
    #[cfg(feature = "owned-relay")]
    Noq {
        allow: Vec<EndpointId>,
        key: Box<rds_net::SecretKey>,
        limits: crate::server::ServerLimits,
    },
}

impl PreparedRelay {
    /// Load or atomically create the owned relay's identity on a blocking worker.
    /// A later startup failure preserves that valid identity for the next retry.
    pub async fn initialize(self) -> Result<ReadyRelay, RelayRuntimeError> {
        let backend = match self.backend {
            PreparedBackend::Iroh { allow, tls } => ReadyBackend::Iroh { allow, tls },
            #[cfg(feature = "owned-relay")]
            PreparedBackend::Noq {
                allow,
                key_file,
                limits,
            } => {
                let key =
                    tokio::task::spawn_blocking(move || rds_net::load_or_create_key(&key_file))
                        .await??;
                ReadyBackend::Noq {
                    allow,
                    key: Box::new(key),
                    limits,
                }
            }
        };
        Ok(ReadyRelay { backend })
    }
}

impl ReadyRelay {
    /// Bind the selected relay after all host configuration has been prepared.
    pub async fn bind(self, addr: SocketAddr) -> Result<RunningRelay, RelayRuntimeError> {
        let backend = match self.backend {
            ReadyBackend::Iroh { allow, tls } => RunningBackend::Iroh(IrohRuntime {
                server: crate::serve_prepared(addr, allow, tls.map(|tls| *tls))
                    .await
                    .map_err(RelayRuntimeError::Startup)?,
                outcome: None,
            }),
            #[cfg(feature = "owned-relay")]
            ReadyBackend::Noq { allow, key, limits } => RunningBackend::Noq(
                crate::server::serve_with_limits(
                    rds_net::EndpointConfig {
                        backend: rds_net::Backend::Noq,
                        secret_key: Some(*key),
                        bind_addrs: vec![addr],
                        discovery: false,
                        ..Default::default()
                    },
                    allow,
                    limits,
                )
                .await
                .map_err(RelayRuntimeError::Startup)?,
            ),
        };
        Ok(RunningRelay { backend })
    }
}

/// Listener addresses and, for the owned protocol, its pinned public identity.
#[derive(Clone, Copy, Debug)]
pub enum RelayBinding {
    Iroh {
        http: SocketAddr,
        https: Option<SocketAddr>,
    },
    Noq {
        addr: SocketAddr,
        id: EndpointId,
    },
}

/// The running relay selected by shared CLI configuration.
pub struct RunningRelay {
    backend: RunningBackend,
}
enum RunningBackend {
    Iroh(IrohRuntime),
    #[cfg(feature = "owned-relay")]
    Noq(crate::server::Relay),
}

struct IrohRuntime {
    server: iroh_relay::server::Server,
    outcome: Option<Result<(), RelayRuntimeError>>,
}

impl IrohRuntime {
    async fn stopped(&mut self) {
        if self.outcome.is_none() {
            let outcome = match self.server.join().await {
                Ok(result) => result.map_err(|e| RelayRuntimeError::IrohShutdown(e.into())),
                Err(error) => Err(RelayRuntimeError::IrohShutdown(error.into())),
            };
            // No await after consuming the upstream JoinHandle: shutdown must
            // return this result instead of polling that same handle again.
            self.outcome = Some(outcome);
        }
    }

    async fn shutdown(self) -> Result<(), RelayRuntimeError> {
        if let Some(outcome) = self.outcome {
            // The upstream supervisor normally joins its workers before exit.
            // Dropping the remaining Server releases its service handles.
            outcome
        } else {
            self.server
                .shutdown()
                .await
                .map_err(|e| RelayRuntimeError::IrohShutdown(e.into()))
        }
    }
}

impl RunningRelay {
    pub fn binding(&self) -> RelayBinding {
        match &self.backend {
            RunningBackend::Iroh(runtime) => RelayBinding::Iroh {
                // serve_prepared always enables the HTTP relay/probe listener.
                http: runtime
                    .server
                    .http_addr()
                    .expect("configured HTTP relay listener"),
                https: runtime.server.https_addr(),
            },
            #[cfg(feature = "owned-relay")]
            RunningBackend::Noq(server) => RelayBinding::Noq {
                addr: server.local_addr(),
                id: server.endpoint_id(),
            },
        }
    }

    /// Observe unexpected completion without initiating shutdown. Cancellation
    /// is safe and repeated observations complete once the runner has stopped.
    /// Always call shutdown afterward: it joins cleanup and returns the retained
    /// failure, including an error observed here. This does not probe reachability.
    pub async fn stopped(&mut self) {
        match &mut self.backend {
            RunningBackend::Iroh(runtime) => runtime.stopped().await,
            #[cfg(feature = "owned-relay")]
            RunningBackend::Noq(server) => {
                // The server retains this result for the subsequent drain/join.
                let _ = server.wait_stopped().await;
            }
        }
    }

    /// Await shutdown, including the owned relay's bounded drain grace.
    /// The host must await this before ending its runtime; Drop cannot join.
    pub async fn shutdown(self) -> Result<(), RelayRuntimeError> {
        match self.backend {
            RunningBackend::Iroh(runtime) => runtime.shutdown().await,
            #[cfg(feature = "owned-relay")]
            RunningBackend::Noq(server) => Ok(server.drain().await?),
        }
    }
}

/// Install supported Unix termination handlers before publishing readiness.
#[cfg(unix)]
pub fn shutdown_signal() -> std::io::Result<impl Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = term.recv() => {}
        }
    })
}

#[cfg(not(unix))]
pub fn shutdown_signal() -> std::io::Result<impl Future<Output = ()>> {
    Ok(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn canceled_iroh_observer_preserves_listener_and_shutdown() {
        let mut relay = RelayArgs::default()
            .prepare()
            .unwrap()
            .initialize()
            .await
            .unwrap()
            .bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(25), relay.stopped())
                .await
                .is_err()
        );
        let RelayBinding::Iroh { http, .. } = relay.binding() else {
            panic!("wrong backend");
        };
        let _socket = tokio::net::TcpStream::connect(http).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), relay.shutdown())
            .await
            .unwrap()
            .unwrap();
        let _rebound = std::net::TcpListener::bind(http).unwrap();
    }

    #[tokio::test]
    async fn irohs_actual_supervisor_error_survives_repeated_observation_and_shutdown() {
        // Upstream starts a supervisor even with no configured service; it
        // returns NoRelayServicesEnabled. This is a real retained engine error.
        let server = iroh_relay::server::Server::spawn(Default::default())
            .await
            .unwrap();
        let mut runtime = IrohRuntime {
            server,
            outcome: None,
        };
        tokio::time::timeout(Duration::from_secs(2), runtime.stopped())
            .await
            .unwrap();
        let before = runtime
            .outcome
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .to_string();
        runtime.stopped().await;
        let error = runtime.shutdown().await.unwrap_err();
        assert_eq!(before, error.to_string());
        assert!(matches!(error, RelayRuntimeError::IrohShutdown(_)));
    }

    #[cfg(feature = "owned-relay")]
    #[tokio::test]
    async fn owned_runtime_observes_completed_relay_without_repolling_its_runner() {
        let server = crate::server::serve(
            rds_net::EndpointConfig {
                backend: rds_net::Backend::Noq,
                discovery: false,
                bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
                ..Default::default()
            },
            vec![],
        )
        .await
        .unwrap();
        server.close().await.unwrap();
        let mut relay = RunningRelay {
            backend: RunningBackend::Noq(server),
        };
        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(2), relay.stopped())
                .await
                .unwrap();
        }
        relay.shutdown().await.unwrap();
    }
}

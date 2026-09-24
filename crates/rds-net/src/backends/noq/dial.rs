//! Bounded initial handshakes; no application work starts on losing candidates.
use std::net::SocketAddr;
use std::time::Duration;
use tokio::task::JoinSet;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub(super) enum DialError {
    #[error("no dial candidates")]
    Empty,
    #[error("initiate candidate handshake: {0}")]
    Connect(#[source] noq::ConnectError),
    #[error("candidate handshake failed: {0}")]
    Handshake(#[source] noq::ConnectionError),
    #[error("candidate handshake timed out")]
    Timeout,
    #[error("candidate task failed: {0}")]
    Task(#[source] tokio::task::JoinError),
}

/// The caller supplies at most eight direct candidates and one attached relay.
/// Every attempt uses the same TLS-pinned server name. A fast failure must not
/// cancel other candidates; only a completed authenticated handshake wins.
pub(super) async fn race(
    endpoint: &noq::Endpoint,
    candidates: &[SocketAddr],
    server_name: &str,
) -> Result<noq::Connection, DialError> {
    let mut attempts = JoinSet::new();
    let mut last_error = None;
    let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
    for &address in candidates {
        match endpoint.connect(address, server_name) {
            Ok(connecting) => {
                attempts.spawn(async move {
                    tokio::time::timeout_at(deadline, connecting)
                        .await
                        .map_err(|_| DialError::Timeout)?
                        .map_err(DialError::Handshake)
                });
            }
            Err(error) => last_error = Some(DialError::Connect(error)),
        }
    }
    while let Some(result) = attempts.join_next().await {
        match result {
            Ok(Ok(winner)) => {
                // Abort pending handshakes and explicitly close any other
                // completed connections before exposing the sole winner.
                attempts.abort_all();
                while let Some(result) = attempts.join_next().await {
                    if let Ok(Ok(loser)) = result {
                        loser.close(0u32.into(), b"another candidate connected");
                    }
                }
                return Ok(winner);
            }
            Ok(Err(error)) => last_error = Some(error),
            Err(error) => last_error = Some(DialError::Task(error)),
        }
    }
    Err(last_error.unwrap_or(DialError::Empty))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::noq as owned;
    use crate::{Backend, EndpointConfig, SecretKey, TransportAddr};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canceling_candidate_race_ends_every_started_handshake() {
        let mut silent = Vec::new();
        for _ in 0..12 {
            silent.push(std::net::UdpSocket::bind("127.0.0.1:0").unwrap());
        }
        let endpoint = owned::bind_endpoint(EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        })
        .await
        .unwrap();
        let target = crate::EndpointAddr {
            id: SecretKey::from_bytes(&[101; 32]).public(),
            addrs: silent
                .iter()
                .map(|s| TransportAddr::Ip(s.local_addr().unwrap()))
                .collect(),
        };
        let task = tokio::spawn({
            let endpoint = endpoint.clone();
            async move { endpoint.connect(target, rds_core::ALPN).await }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while endpoint.inner.stats().outgoing_handshakes < owned::policy::MAX_CANDIDATES as u64
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            endpoint.inner.stats().outgoing_handshakes,
            owned::policy::MAX_CANDIDATES as u64
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        // Observe the QUIC engine, not only our policy task count. Retain the
        // endpoint so dropping the last endpoint cannot hide a leaked attempt.
        // QUIC closing retains packet state for three initial PTOs (~3 s),
        // so this observes engine draining, not synchronous task destruction.
        tokio::time::timeout(Duration::from_secs(5), endpoint.inner.wait_all_draining())
            .await
            .unwrap();
        assert_eq!(endpoint.active_path_drivers(), 0);
        endpoint.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn racing_live_addresses_exposes_only_one_connection() {
        let client = owned::bind_endpoint(EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        })
        .await
        .unwrap();
        let server = owned::bind_endpoint(EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            bind_addrs: vec![
                "127.0.0.1:0".parse().unwrap(),
                "127.0.0.1:0".parse().unwrap(),
            ],
            ..Default::default()
        })
        .await
        .unwrap();
        let incoming = tokio::spawn({
            let server = server.clone();
            async move {
                let first = server.accept().await.unwrap();
                let second = server.accept().await.unwrap();
                let (a, b) = tokio::join!(
                    tokio::time::timeout(Duration::from_secs(2), first),
                    tokio::time::timeout(Duration::from_secs(2), second)
                );
                [a, b]
                    .into_iter()
                    .filter_map(|result| result.ok().and_then(Result::ok))
                    .collect::<Vec<_>>()
            }
        });
        let conn = tokio::time::timeout(
            Duration::from_secs(3),
            client.connect(server.addr(), rds_core::ALPN),
        )
        .await
        .unwrap()
        .unwrap();
        let accepted = tokio::time::timeout(Duration::from_secs(3), incoming)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while accepted
                .iter()
                .filter(|c| c.inner().close_reason().is_none())
                .count()
                != 1
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let selected = accepted
            .iter()
            .find(|c| c.inner().close_reason().is_none())
            .unwrap();
        conn.send_datagram(b"sole winner".to_vec().into()).unwrap();
        assert_eq!(
            &tokio::time::timeout(Duration::from_secs(2), selected.read_datagram())
                .await
                .unwrap()
                .unwrap()[..],
            b"sole winner"
        );
        assert_eq!(client.active_path_drivers(), 1);
        client.close().await;
        server.close().await;
        assert_eq!(client.active_path_drivers(), 0);
        assert_eq!(server.active_path_drivers(), 0);
    }
}

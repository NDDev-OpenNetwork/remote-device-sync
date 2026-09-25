use super::*;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

#[cfg(target_os = "linux")]
#[path = "tests/special_file_linux.rs"]
mod special_file;
#[cfg(all(unix, not(target_os = "linux")))]
#[path = "tests/special_file_unix.rs"]
mod special_file;

struct Fixture {
    dir: PathBuf,
    token: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("rds-admin-{:032x}", rand::random::<u128>()));
        std::fs::create_dir(&dir).unwrap();
        Token::create(&dir.join("token")).unwrap();
        let token = std::fs::read_to_string(dir.join("token")).unwrap();
        Self { dir, token }
    }
    async fn prepared(&self) -> Prepared {
        Prepared::bind(
            "127.0.0.1:0".parse().unwrap(),
            Token::load(&self.dir.join("token")).unwrap(),
        )
        .await
        .unwrap()
    }
    fn request(&self, path: &str) -> String {
        format!(
            "GET {path} HTTP/1.1\r\nHost: fixture.invalid\r\nAuthorization: Bearer {}\r\n\r\n",
            self.token
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn request(addr: SocketAddr, text: &str) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(text.as_bytes()).await.unwrap();
        let mut response = String::new();
        if let Err(error) = client.read_to_string(&mut response).await {
            // Refusing unread trailing bytes may close with TCP RST. The
            // already-complete HTTP response must still have its exact length.
            assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
        }
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(body.len(), length);
        response
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn authenticated_scrape_has_unique_family_types_and_no_unauthenticated_source_access() {
    let fixture = Fixture::new();
    let calls = Arc::new(AtomicU64::new(0));
    let count = calls.clone();
    let mut server = Server::start(Some(fixture.prepared().await), move || {
        count.fetch_add(1, Ordering::Relaxed);
        Snapshot::from([
            ("rds_fixture_bytes_total{via=\"direct\"}", 123),
            ("rds_fixture_bytes_total{via=\"relay\"}", 456),
            ("rds_fixture_active", 7),
        ])
    });
    let addr = server.addr().unwrap();
    for fields in [
        String::new(),
        "Authorization: Bearer wrong\r\n".into(),
        format!(
            "X-Forwarded-For: 127.0.0.1\r\nProxy-Authorization: Bearer {}\r\n",
            fixture.token
        ),
    ] {
        let response = request(
            addr,
            &format!("GET /metrics HTTP/1.1\r\nHost: fixture.invalid\r\n{fields}\r\n"),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 401"), "{response}");
        assert!(!response.contains("rds_"));
    }
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    let response = request(addr, &fixture.request("/metrics")).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert_eq!(
        response
            .matches("# TYPE rds_fixture_bytes_total counter")
            .count(),
        1
    );
    assert!(response.contains("rds_fixture_bytes_total{via=\"direct\"} 123\n"));
    assert!(response.contains("# TYPE rds_fixture_active gauge\n"));
    assert!(response.contains("rds_admin_unauthorized_total 3\n"));
    assert!(!response.contains(&fixture.token));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    // TCP may split the write exactly at the first header boundary. The server
    // may reject buffered trailing bytes or return the first response, but can
    // never process a second request on this connection.
    let pipelined = format!(
        "{}{}",
        fixture.request("/metrics"),
        fixture.request("/metrics")
    );
    let response = request(addr, &pipelined).await;
    assert_eq!(response.matches("HTTP/1.1 ").count(), 1);
    assert!(calls.load(Ordering::Relaxed) <= 2);

    server.close().await.unwrap();
    assert_eq!(server.snapshot()["rds_admin_connections_active"], 0);
    assert!(TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn ambiguous_framing_never_authorizes_a_metrics_request() {
    let fixture = Fixture::new();
    let mut server = Server::start(Some(fixture.prepared().await), || {
        panic!("rejected request invoked source")
    });
    let addr = server.addr().unwrap();
    for fields in [
        format!("Authorization: Bearer {}\r\n", fixture.token),
        "Content-Length: 1\r\n".into(),
        "Content-Length: 0\r\nContent-Length: 0\r\n".into(),
        "Transfer-Encoding: chunked\r\n".into(),
        "Transfer-Encoding: chunked\r\nContent-Length: 0\r\n".into(),
        "Host: another.invalid\r\n".into(),
        "Authorization : invalid\r\n".into(),
        "Expect: 100-continue\r\n".into(),
        "Upgrade: websocket\r\n".into(),
        "Origin: https://fixture.invalid\r\n".into(),
    ] {
        let text = format!(
            "GET /metrics HTTP/1.1\r\nHost: fixture.invalid\r\nAuthorization: Bearer {}\r\n{fields}\r\n",
            fixture.token
        );
        let response = request(addr, &text).await;
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    }
    for path in [
        "/metrics?token=bad",
        "/v1/metrics",
        "http://fixture.invalid/metrics",
        "/metrics/",
        "/%6detrics",
    ] {
        assert!(
            request(addr, &fixture.request(path))
                .await
                .starts_with("HTTP/1.1 404")
        );
    }
    assert!(
        request(addr, &fixture.request("/metrics").replace("GET ", "HEAD "))
            .await
            .starts_with("HTTP/1.1 405")
    );
    let huge = format!(
        "GET /metrics HTTP/1.1\r\nX-Fill: {}",
        "x".repeat(http::MAX_HEADER)
    );
    assert!(request(addr, &huge).await.starts_with("HTTP/1.1 431"));
    server.close().await.unwrap();
}

#[tokio::test]
async fn saturation_and_stalled_headers_release_capacity_with_bounded_shutdown() {
    let fixture = Fixture::new();
    let mut server = Server::start(Some(fixture.prepared().await), Snapshot::new);
    let addr = server.addr().unwrap();
    let mut held = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        held.push(TcpStream::connect(addr).await.unwrap());
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.snapshot()["rds_admin_connections_active"] != MAX_CONNECTIONS as u64 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut extra = TcpStream::connect(addr).await.unwrap();
    let mut byte = [0];
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), extra.read(&mut byte))
            .await
            .unwrap(),
        Ok(0) | Err(_)
    ));
    assert_eq!(server.snapshot()["rds_admin_connections_rejected_total"], 1);
    tokio::time::timeout(Duration::from_secs(4), async {
        while server.snapshot()["rds_admin_connections_active"] != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        server.snapshot()["rds_admin_timeouts_total"],
        MAX_CONNECTIONS as u64
    );
    assert!(
        request(addr, &fixture.request("/metrics"))
            .await
            .starts_with("HTTP/1.1 200")
    );
    let mut stalled = TcpStream::connect(addr).await.unwrap();
    stalled
        .write_all(b"GET /metrics HTTP/1.1\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), server.close())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(stalled.read(&mut byte).await, Ok(0) | Err(_)));
    assert_eq!(server.snapshot()["rds_admin_connections_active"], 0);
}

#[tokio::test]
async fn canceled_observers_repeated_close_and_drop_do_not_detach_listeners() {
    let fixture = Fixture::new();
    let mut server = Server::start(Some(fixture.prepared().await), Snapshot::new);
    let addr = server.addr().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), server.stopped())
            .await
            .is_err()
    );
    assert!(
        request(addr, &fixture.request("/metrics"))
            .await
            .starts_with("HTTP/1.1 200")
    );
    server.close().await.unwrap();
    server.close().await.unwrap();
    server.stopped().await.unwrap();
    let server = Server::start(Some(fixture.prepared().await), Snapshot::new);
    let addr = server.addr().unwrap();
    let counters = server.counters.clone();
    let mut client = TcpStream::connect(addr).await.unwrap();
    client.write_all(b"GET ").await.unwrap();
    drop(server);
    tokio::time::timeout(Duration::from_secs(1), async {
        let mut byte = [0];
        assert!(matches!(client.read(&mut byte).await, Ok(0) | Err(_)));
        while counters.active.load(Ordering::Relaxed) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn invalid_snapshot_is_fail_closed_and_panicked_source_is_supervised() {
    let fixture = Fixture::new();
    let mut server = Server::start(Some(fixture.prepared().await), || {
        Snapshot::from([("rds_bad\nPRIVATE_SENTINEL", 1)])
    });
    let response = request(server.addr().unwrap(), &fixture.request("/metrics")).await;
    assert!(response.starts_with("HTTP/1.1 500"));
    assert!(!response.contains("PRIVATE_SENTINEL"));
    assert_eq!(server.snapshot()["rds_admin_snapshot_errors_total"], 1);
    server.close().await.unwrap();
    let mut server = Server::start(Some(fixture.prepared().await), || {
        panic!("fixture source panic")
    });
    let mut stream = TcpStream::connect(server.addr().unwrap()).await.unwrap();
    stream
        .write_all(fixture.request("/metrics").as_bytes())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), server.stopped())
            .await
            .unwrap()
            .is_err()
    );
    assert!(server.close().await.is_err());
    assert!(server.stopped().await.is_err());
    assert_eq!(server.snapshot()["rds_admin_connections_active"], 0);
}

#[test]
fn exposition_refuses_private_labels_and_excessive_cardinality() {
    for name in [
        "rds_metric{peer=\"PRIVATE_SENTINEL\"}",
        "rds_metric{via=\"unknown\"}",
        "rds_metric{via=\"direct\",peer=\"PRIVATE_SENTINEL\"}",
        "foreign_metric",
    ] {
        assert!(snapshot::render(Snapshot::from([(name, 1)])).is_none());
    }
    let mut oversized = Snapshot::new();
    for i in 0..=snapshot::MAX_SAMPLES {
        // Synthetic static metadata, bounded to one test-sized allocation.
        let name: &'static str = Box::leak(format!("rds_fixture_{i}").into_boxed_str());
        oversized.insert(name, 1);
    }
    assert!(snapshot::render(oversized).is_none());
}

#[tokio::test]
async fn public_binds_are_refused_and_disabled_admin_has_no_task() {
    let fixture = Fixture::new();
    for addr in ["0.0.0.0:0", "[::]:0", "192.0.2.1:0", "[::ffff:127.0.0.1]:0"] {
        assert!(matches!(
            Prepared::bind(
                addr.parse().unwrap(),
                Token::load(&fixture.dir.join("token")).unwrap()
            )
            .await,
            Err(Error::Configuration)
        ));
    }
    let mut disabled = Server::start(None, || panic!("disabled source"));
    assert!(disabled.addr().is_none() && disabled.task.is_none());
    assert!(
        tokio::time::timeout(Duration::from_millis(10), disabled.stopped())
            .await
            .is_err()
    );
    disabled.close().await.unwrap();
}

#[cfg(unix)]
#[test]
fn credential_files_reject_links_permissions_special_files_and_bad_content() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    let fixture = Fixture::new();
    let path = fixture.dir.join("token");
    assert!(Token::load(&path).is_ok());
    assert!(Token::create(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), fixture.token);
    symlink(&path, fixture.dir.join("link")).unwrap();
    assert!(Token::load(&fixture.dir.join("link")).is_err());
    std::fs::hard_link(&path, fixture.dir.join("hard")).unwrap();
    assert!(Token::load(&path).is_err());
    std::fs::remove_file(fixture.dir.join("hard")).unwrap();
    for mode in [0o644, 0o640, 0o700, 0o200] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(Token::load(&path).is_err());
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    for content in [
        String::new(),
        "a".repeat(63),
        "a".repeat(66),
        "A".repeat(64),
        format!("{}\n\n", fixture.token),
        format!("{}\r\n", fixture.token),
    ] {
        std::fs::write(&path, content).unwrap();
        assert!(Token::load(&path).is_err());
    }
    std::fs::write(&path, format!("{}\n", fixture.token)).unwrap();
    assert!(Token::load(&path).is_ok());
    assert!(Token::load(&fixture.dir).is_err());
    let special = fixture.dir.join("special");
    special_file::create(&special);
    assert!(Token::load(&special).is_err());
}

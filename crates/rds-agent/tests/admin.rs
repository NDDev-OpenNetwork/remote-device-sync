#![cfg(unix)]
const BINARY: &str = env!("CARGO_BIN_EXE_rds-agent");
const ROLE: &str = "agent";
const OWNED: bool = cfg!(feature = "transport-noq");
include!("../../../tests/support/admin_cli.rs");

#[cfg(not(feature = "transport-noq"))]
#[tokio::test]
async fn unavailable_iroh_relay_does_not_block_local_service_or_signal_shutdown() {
    // Own the port but never answer the relay handshake. No external service,
    // port-selection race or WAN timeout controls the daemon's local readiness.
    let stalled = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = rds_net::bind_endpoint(rds_net::EndpointConfig {
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        discovery: false,
        ..Default::default()
    })
    .await
    .unwrap();
    let fixture = Fixture::new();
    let stdout = fixture.0.join("stdout");
    let mut child = Process(
        fixture
            .command_with_relay(Some(stalled.local_addr().unwrap()))
            .args(["--allow", &client.id().to_string()])
            .args(["--admin-addr", "127.0.0.1:0", "--admin-token-file"])
            .arg(fixture.0.join("token"))
            .stdout(std::fs::File::create(&stdout).unwrap())
            .spawn()
            .unwrap(),
    );
    let (ticket, admin) = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(child.0.try_wait().unwrap().is_none(), "{}", fixture.log());
            let output = std::fs::read_to_string(&stdout).unwrap();
            let log = fixture.log();
            if let Some(ticket) = output
                .lines()
                .find_map(|line| line.strip_prefix("ticket: "))
                && let Some(admin) = log
                    .lines()
                    .find(|line| line.contains("admin metrics listening"))
            {
                break (ticket.parse::<rds_net::Ticket>().unwrap(), address(admin));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|error| panic!("local startup timed out: {error}; {}", fixture.log()));
    // Exercise the actual allowed product path, not just admin readiness.
    let conn = tokio::time::timeout(Duration::from_secs(5), rds_cli::connect(&client, ticket.0))
        .await
        .unwrap()
        .unwrap();
    rds_cli::ping(&conn, 41).await.unwrap();
    let credential = std::fs::read_to_string(fixture.0.join("token")).unwrap();
    let metrics = wire(
        admin,
        &format!(
            "GET /metrics HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {credential}\r\n\r\n"
        ),
    )
    .await;
    assert!(metrics.starts_with("HTTP/1.1 200"));
    assert!(metrics.contains("rds_net_connections_accepted_total 1\n"));
    client.close().await;
    let pid = rustix::process::Pid::from_raw(child.0.id().try_into().unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    assert!(exit(&mut child).await.success(), "{}", fixture.log());
    assert!(TcpStream::connect(admin).await.is_err());
}

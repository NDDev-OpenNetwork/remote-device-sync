const BINARY: &str = env!("CARGO_BIN_EXE_rds");
const TAIL: &[&str] = &["id"];
include!("../../../tests/support/endpoint_cli.rs");

#[test]
fn invalid_tcp_destination_fails_before_identity_or_dial() {
    for command in ["ssh", "forward"] {
        for target in [":22", "host:0", "host:nope", "::1:22", "[::1]22"] {
            let dir = Scratch::new();
            let output = run_with_tail(
                &["--no-relay", command, "unused-peer", "--remote", target],
                &[],
                &dir,
            );
            assert!(!output.status.success());
            assert!(!dir.0.join("endpoint.key").exists(), "{command}/{target}");
        }
    }
}

#[test]
fn id_uses_valid_file_and_explicit_overrides_without_network() {
    let dir = Scratch::new();
    let config = dir.0.join("endpoint.json");
    std::fs::write(
        &config,
        include_bytes!("../../../examples/endpoint-iroh.json"),
    )
    .unwrap();
    let output = run(
        &["--endpoint-config", config.to_str().unwrap(), "--no-relay"],
        &dir,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let id: rds_net::EndpointId = std::str::from_utf8(&output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let seed: [u8; 32] = std::fs::read(dir.0.join("endpoint.key"))
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(id, rds_net::SecretKey::from_bytes(&seed).public());
}

#[test]
fn invalid_forward_budget_fails_before_identity_or_dial() {
    for command in ["ssh", "forward"] {
        for limit in ["0", "65536", "-1", "unlimited"] {
            let dir = Scratch::new();
            let output = run_with_tail(
                &[
                    "--no-relay",
                    command,
                    "unused-peer",
                    "--remote",
                    "127.0.0.1:22",
                    "--max-connections",
                    limit,
                ],
                &[],
                &dir,
            );
            assert!(!output.status.success());
            assert!(!dir.0.join("endpoint.key").exists(), "{command}/{limit}");
        }
    }
}

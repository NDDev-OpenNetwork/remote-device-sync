const BINARY: &str = env!("CARGO_BIN_EXE_rds-agent");
const TAIL: &[&str] = &[];
include!("../../../tests/support/endpoint_cli.rs");

#[test]
fn invalid_ssh_destination_fails_before_identity_or_bind() {
    for target in [":22", "host:0", "host:nope", "::1:22", "[::1]22"] {
        let dir = Scratch::new();
        let output = run(&["--no-relay", "--ssh", target], &dir);
        assert!(!output.status.success());
        assert!(!dir.0.join("endpoint.key").exists(), "{target}");
    }
}

#[test]
fn invalid_admission_budget_fails_before_identity_or_bind() {
    for flag in ["--max-connections", "--max-streams"] {
        for value in ["0", "65536", "-1", "unlimited"] {
            let dir = Scratch::new();
            let output = run(&["--no-relay", flag, value], &dir);
            assert!(!output.status.success());
            assert!(!dir.0.join("endpoint.key").exists(), "{flag} {value}");
        }
    }
}

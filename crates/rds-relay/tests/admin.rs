#![cfg(unix)]
const BINARY: &str = env!("CARGO_BIN_EXE_rds-relay");
const ROLE: &str = "relay";
const OWNED: bool = cfg!(feature = "owned-relay");
include!("../../../tests/support/admin_cli.rs");

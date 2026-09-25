#![cfg(unix)]
const BINARY: &str = env!("CARGO_BIN_EXE_rds-relay");
const BIND_FLAG: &str = "--addr";
const HOSTS_DIRECTORY: bool = false;
include!("../../../tests/support/relay_runtime.rs");

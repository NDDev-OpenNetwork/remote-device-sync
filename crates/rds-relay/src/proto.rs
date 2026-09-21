//! Owned relay protocol — wire types live in `rds_core::relay` so both
//! the relay server (`rds-relay`) and the client transport (`rds-net`)
//! can use them without a dependency cycle. Re-exported here.

pub use rds_core::relay::{
    KEY_HEADER_LEN, Key, MAX_PAYLOAD, RELAY_ALPN, RelayControl, decode_frame, encode_forward,
};

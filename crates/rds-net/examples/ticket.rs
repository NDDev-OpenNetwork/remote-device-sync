//! Mint an `rds1` ticket with a restricted address set — e.g. a
//! relay-only ticket to prove the relay path on real networks.
//!
//! Usage: ticket <endpoint-id-hex> [relay-url] [ip:port ...]

use std::collections::BTreeSet;
use std::str::FromStr;

use rds_net::{EndpointAddr, EndpointId, RelayUrl, Ticket, TransportAddr};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let id = EndpointId::from_str(&args.next().ok_or_else(|| {
        anyhow::anyhow!("usage: ticket <endpoint-id-hex> [relay-url] [ip:port ...]")
    })?)?;
    let mut addrs = BTreeSet::new();
    for a in args {
        if let Ok(url) = a.parse::<RelayUrl>() {
            addrs.insert(TransportAddr::Relay(url));
        } else {
            let sa: std::net::SocketAddr = a.parse()?;
            addrs.insert(TransportAddr::Ip(sa));
        }
    }
    println!("{}", Ticket(EndpointAddr { id, addrs }));
    Ok(())
}

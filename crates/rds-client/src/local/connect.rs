//! One reserved peer association, with bounded optional direct-address probing.
use super::*;
use rds_net::EndpointAddr;

pub(super) async fn run(
    shared: &state::Shared,
    endpoint: &Endpoint,
    directory: Option<rds_discovery::client::Client>,
    target: String,
    grant: Option<Box<rds_core::grant::Grant>>,
    local_addresses: Option<Vec<std::net::SocketAddr>>,
) -> Result<Output, ErrorCode> {
    if target.len() > 8192 {
        return Err(ErrorCode::InvalidRequest);
    }
    let pinned = rds_net::parse_target(&target).ok();
    if let Some(addresses) = &local_addresses {
        rds_core::local::validate_local_addresses(addresses)?;
        if pinned.is_none() {
            return Err(ErrorCode::InvalidRequest);
        }
    }
    if pinned.as_ref().is_some_and(|addr| addr.id == endpoint.id()) {
        return Err(ErrorCode::InvalidRequest);
    }
    let credential = grant
        .as_ref()
        .map(|g| postcard::to_stdvec(g).map(|b| *blake3::hash(&b).as_bytes()))
        .transpose()
        .map_err(|_| ErrorCode::InvalidRequest)?;
    if let Some(pinned) = &pinned
        && let Some(existing) = state::existing(shared, pinned.id, credential)?
    {
        return reused(existing, shared).await;
    }
    // Preferred attempts reserve the pinned identity BEFORE network or
    // directory I/O, so monitor windows share one in-flight dial.
    let resolved = if local_addresses.is_none() {
        Some(
            rds_net::resolve_target(directory.clone(), &target)
                .await
                .map_err(|_| ErrorCode::Resolve)?,
        )
    } else {
        None
    };
    let peer = resolved
        .as_ref()
        .or(pinned.as_ref())
        .ok_or(ErrorCode::InvalidRequest)?
        .id;
    if peer == endpoint.id() {
        return Err(ErrorCode::InvalidRequest);
    }
    let reservation = match state::reserve(shared, peer, credential)? {
        state::Reserved::New(reservation) => reservation,
        other => return reused(other, shared).await,
    };
    let mut guard = tokio::select! {
        biased;
        _ = reservation.cancel.cancelled() => return Err(ErrorCode::NotFound),
        result = async {
            let direct = if let Some(addresses) = local_addresses.filter(|v| !v.is_empty()) {
                let addr = EndpointAddr::from_parts(peer, addresses.iter().copied().map(rds_net::TransportAddr::Ip));
                // One budget for the entire candidate set, not one per address.
                let connected = tokio::time::timeout(Duration::from_secs(1), endpoint.connect(addr, rds_core::ALPN))
                    .await.ok().and_then(Result::ok);
                if connected.is_none() {
                    // A wrong-key hint must not remain in Iroh's shared cache
                    // and win the next handshake against the directory route.
                    tokio::time::timeout(Duration::from_secs(1),
                        endpoint.discard_unvalidated_local_addresses(peer, addresses))
                        .await.map_err(|_| ErrorCode::Connect)?;
                }
                connected
            } else { None };
            let conn = match direct {
                Some(conn) => conn,
                None => {
                    let addr = match resolved {
                        Some(addr) => addr,
                        None => rds_net::resolve_target(directory, &target).await.map_err(|_| ErrorCode::Resolve)?,
                    };
                    if addr.id != peer { return Err(ErrorCode::Resolve); }
                    crate::connect(endpoint, addr).await.map_err(|_| ErrorCode::Connect)?
                },
            };
            let guard = DialGuard(Some(conn));
            // Dial success already authenticates the pinned endpoint key.
            // Admission failure is final; it never retries another route.
            let guard = match grant {
                Some(grant) => {
                    let mut guard = guard;
                    let conn = crate::authorize_connection(guard.0.take().ok_or(ErrorCode::Internal)?, &grant)
                        .await.map_err(|_| ErrorCode::Connect)?;
                    DialGuard(Some(conn))
                },
                None => {
                    crate::ping(guard.0.as_ref().ok_or(ErrorCode::Internal)?, rand::random())
                        .await.map_err(|_| ErrorCode::Connect)?;
                    guard
                },
            };
            Ok::<_, ErrorCode>(guard)
        } => result?,
    };
    let mut state = state::lock(shared)?;
    let entry = state
        .entries
        .get_mut(&reservation.id)
        .ok_or(ErrorCode::NotFound)?;
    let conn = guard.0.take().ok_or(ErrorCode::Internal)?;
    entry.sampler = Some(endpoint.metrics().sampler(conn.clone()));
    entry.conn = Some(conn);
    if state.selected.is_none() {
        state.selected = Some(reservation.id);
    }
    state.changed();
    drop(state);
    Ok(Output {
        reply: Reply::Connected(reservation.id),
        reservation: Some(reservation),
        tcp: None,
        desktop: None,
        events: None,
    })
}

async fn reused(reserved: state::Reserved, shared: &state::Shared) -> Result<Output, ErrorCode> {
    match reserved {
        state::Reserved::Existing(id) => Ok(Output::reply(Reply::Connected(id))),
        state::Reserved::Pending(pending) => {
            Ok(Output::reply(Reply::Connected(pending.wait(shared).await?)))
        }
        state::Reserved::New(_) => Err(ErrorCode::Internal),
    }
}

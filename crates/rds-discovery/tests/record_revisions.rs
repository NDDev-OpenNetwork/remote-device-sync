use ed25519_dalek::{Signer, SigningKey};
use rds_discovery::{
    DeleteRequest, DiscoveryError, EndpointKey, EndpointRecord, FileStore, MAX_DIRECT_ADDRS,
    MAX_RECORD_BYTES, MAX_RECORD_TTL, MemoryStore, Payload, RecordStore, Service, now_unix,
    publisher::{RecordDraft, RecordIssuer},
};
use std::{path::PathBuf, time::Duration};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("rds-revisions-{:032x}", rand::random::<u128>()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn signer() -> SigningKey {
    SigningKey::from_bytes(&[84; 32])
}
fn draft(port: u16) -> RecordDraft {
    RecordDraft {
        addrs: vec![([127, 0, 0, 1], port).into()],
        relay_urls: vec![],
        services: vec![Service::Ping],
        ttl: Duration::from_secs(300),
    }
}
fn raw_signed(payload: &Payload) -> EndpointRecord {
    let bytes = postcard::to_stdvec(payload).unwrap();
    let signature = signer()
        .sign(&[b"rds/endpoint-record/v1\0".as_slice(), &bytes].concat())
        .to_bytes()
        .to_vec();
    EndpointRecord {
        key: payload.key,
        payload: bytes,
        signature,
    }
}

#[test]
fn same_second_revisions_exact_retry_and_delete_order_agree_in_both_stores() {
    let tmp = Temp::new();
    for store in [
        Box::new(MemoryStore::default()) as Box<dyn RecordStore>,
        Box::new(FileStore::new(&tmp.0).unwrap()),
    ] {
        let mut issuer = RecordIssuer::memory(signer());
        let now = now_unix().unwrap();
        let first = issuer.record(draft(4000), now).unwrap();
        let second = issuer.record(draft(4001), now).unwrap();
        assert_eq!(
            first.verify().unwrap().issued_at,
            second.verify().unwrap().issued_at
        );
        assert_eq!(second.verify().unwrap().revision, 2);
        store.put(&first).unwrap();
        store.put(&second).unwrap();
        store.put(&second).unwrap();
        assert_eq!(store.get(&issuer.key()).unwrap(), second);
        let mut conflict = second.verify().unwrap();
        conflict.addrs = vec!["127.0.0.1:4999".parse().unwrap()];
        assert!(matches!(
            store.put(&EndpointRecord::sign(&conflict, &signer()).unwrap()),
            Err(DiscoveryError::Stale)
        ));
        assert!(matches!(store.put(&first), Err(DiscoveryError::Stale)));
        let tomb = issuer.delete(now).unwrap();
        assert_eq!(tomb.verify().unwrap().revision, 3);
        store.remove(&tomb).unwrap();
        store.remove(&tomb).unwrap();
        assert!(matches!(store.put(&second), Err(DiscoveryError::Stale)));
        assert!(matches!(
            store.get(&issuer.key()),
            Err(DiscoveryError::NotFound)
        ));
        let fourth = issuer.record(draft(4002), now).unwrap();
        store.put(&fourth).unwrap();
        assert!(matches!(store.remove(&tomb), Err(DiscoveryError::Stale)));
        assert_eq!(store.get(&issuer.key()).unwrap(), fourth);
    }
}

#[test]
fn exact_disk_retry_does_not_allocate_a_commit_or_extend_validity() {
    let tmp = Temp::new();
    let store = FileStore::new(&tmp.0).unwrap();
    let mut issuer = RecordIssuer::memory(signer());
    let record = issuer.record(draft(4000), now_unix().unwrap()).unwrap();
    store.put(&record).unwrap();
    let anchor = std::fs::read(tmp.0.join("records.anchor")).unwrap();
    store.put(&record).unwrap();
    assert_eq!(std::fs::read(tmp.0.join("records.anchor")).unwrap(), anchor);
    assert_eq!(store.get(&issuer.key()).unwrap(), record);
}

#[test]
fn validity_has_exact_boundaries_and_never_substitutes_for_revision() {
    let mut issuer = RecordIssuer::memory(signer());
    let rec = issuer.record(draft(4000), 1000).unwrap();
    assert!(rec.verify_fresh_at(999).is_err());
    assert!(rec.verify_fresh_at(1000).is_ok());
    assert!(rec.verify_fresh_at(1299).is_ok());
    assert!(matches!(
        rec.verify_fresh_at(1300),
        Err(DiscoveryError::Expired)
    ));
    let tomb = issuer.delete(1000).unwrap();
    assert!(tomb.verify_fresh_at(999).is_err());
    assert!(tomb.verify_fresh_at(1300).is_err());
    let valid = rec.verify().unwrap();
    for (version, revision, issued, expires) in [
        (2, 1, 1000, 1300),
        (1, 0, 1000, 1300),
        (1, 1, 1000, 1000),
        (1, 1, 1000, 999),
        (1, 1, 1000, 1001 + MAX_RECORD_TTL),
    ] {
        let payload = Payload {
            version,
            revision,
            issued_at: issued,
            expires_at: expires,
            ..valid.clone()
        };
        assert!(EndpointRecord::sign(&payload, &signer()).is_err());
        assert!(raw_signed(&payload).verify().is_err());
    }
}

#[test]
fn signatures_bind_domain_identity_and_exact_payload_before_decoding() {
    let mut issuer = RecordIssuer::memory(signer());
    let rec = issuer.record(draft(4000), 1000).unwrap();
    let tomb = issuer.delete(1000).unwrap();
    let wrong_domain = EndpointRecord {
        key: tomb.key,
        payload: tomb.payload,
        signature: tomb.signature,
    };
    assert!(matches!(
        wrong_domain.verify(),
        Err(DiscoveryError::BadSignature)
    ));
    let mut bad = rec.clone();
    bad.key = EndpointKey(SigningKey::from_bytes(&[85; 32]).verifying_key().to_bytes());
    assert!(matches!(bad.verify(), Err(DiscoveryError::BadSignature)));
    bad = rec.clone();
    bad.payload = vec![0xff; 100];
    assert!(
        matches!(bad.verify(), Err(DiscoveryError::BadSignature)),
        "untrusted payload was decoded first"
    );
    bad = rec;
    bad.payload.push(0);
    bad.signature = signer()
        .sign(&[b"rds/endpoint-record/v1\0".as_slice(), &bad.payload].concat())
        .to_bytes()
        .to_vec();
    assert!(bad.verify().is_err(), "signed trailing bytes accepted");
}

#[test]
fn collection_and_wire_bounds_hold_even_for_a_valid_signer() {
    let mut issuer = RecordIssuer::memory(signer());
    let valid = issuer.record(draft(4000), 1000).unwrap();
    let payload = valid.verify().unwrap();
    let mut excessive = payload.clone();
    excessive.addrs = (1..=MAX_DIRECT_ADDRS + 1)
        .map(|port| ([127, 0, 0, 1], port as u16).into())
        .collect();
    assert!(raw_signed(&excessive).verify().is_err());
    excessive = payload.clone();
    excessive.services = vec![Service::Ping; 7];
    assert!(raw_signed(&excessive).verify().is_err());
    for url in [
        "file:///tmp/relay".into(),
        "https://user:password@relay.example/".into(),
        "https://relay.example/?token=synthetic".into(),
        format!("https://{}.example/", "a".repeat(600)),
    ] {
        excessive = payload.clone();
        excessive.relay_urls = vec![url];
        assert!(raw_signed(&excessive).verify().is_err());
    }
    let mut json = serde_json::to_value(&valid).unwrap();
    json["payload"] = serde_json::json!(vec![0u8; MAX_RECORD_BYTES + 1]);
    assert!(serde_json::from_value::<EndpointRecord>(json).is_err());
    let mut wrong_key = payload;
    wrong_key.key = EndpointKey([0; 32]);
    assert!(EndpointRecord::sign(&wrong_key, &signer()).is_err());
}

#[test]
fn issuer_restart_preserves_exact_bytes_and_one_shared_sequence() {
    let tmp = Temp::new();
    let mut issuer = RecordIssuer::open(&tmp.0, signer(), 1000).unwrap();
    assert!(RecordIssuer::open(&tmp.0, signer(), 1000).is_err());
    let first = issuer.record(draft(4000), 1000).unwrap();
    drop(issuer);
    let mut issuer = RecordIssuer::open(&tmp.0, signer(), 1001).unwrap();
    assert_eq!(issuer.record(draft(4000), 1001).unwrap(), first);
    let second = issuer.record(draft(4001), 1001).unwrap();
    assert_eq!(second.verify().unwrap().revision, 2);
    let tomb = issuer.delete(1001).unwrap();
    drop(issuer);
    let mut issuer = RecordIssuer::open(&tmp.0, signer(), 1002).unwrap();
    assert_eq!(issuer.delete(1002).unwrap(), tomb);
    assert_eq!(
        issuer
            .record(draft(4001), 1002)
            .unwrap()
            .verify()
            .unwrap()
            .revision,
        4
    );
    assert!(issuer.record(draft(4001), 1001).is_err());
    drop(issuer);
    assert!(RecordIssuer::open(&tmp.0, signer(), 1001).is_err());
    assert!(RecordIssuer::open(&tmp.0, SigningKey::from_bytes(&[85; 32]), 1003).is_err());
    std::fs::remove_file(tmp.0.join("publisher.json")).unwrap();
    assert!(RecordIssuer::open(&tmp.0, signer(), 1003).is_err());
}

#[test]
fn future_or_expired_mutations_are_refused_at_store_boundary() {
    let tmp = Temp::new();
    for store in [
        Box::new(MemoryStore::default()) as Box<dyn RecordStore>,
        Box::new(FileStore::new(&tmp.0).unwrap()),
    ] {
        let now = now_unix().unwrap();
        for issued in [now - 1000, now + 1000] {
            let mut issuer = RecordIssuer::memory(signer());
            assert!(
                store
                    .put(&issuer.record(draft(4000), issued).unwrap())
                    .is_err()
            );
            assert!(store.remove(&issuer.delete(issued).unwrap()).is_err());
        }
        let mut issuer = RecordIssuer::memory(signer());
        store
            .put(&issuer.record(draft(4000), now).unwrap())
            .unwrap();
        assert_eq!(store.len(), 1);
    }
}

#[test]
fn issuer_renewal_is_bounded_and_invalid_draft_does_not_consume_revision() {
    let mut issuer = RecordIssuer::memory(signer());
    let first = issuer.record(draft(4000), 1000).unwrap();
    assert_eq!(issuer.record(draft(4000), 1099).unwrap(), first);
    let second = issuer.record(draft(4000), 1100).unwrap();
    assert_eq!(second.verify().unwrap().revision, 2);
    let mut invalid = draft(4000);
    invalid.ttl = Duration::ZERO;
    assert!(issuer.record(invalid, 1100).is_err());
    assert_eq!(
        issuer
            .record(draft(4001), 1100)
            .unwrap()
            .verify()
            .unwrap()
            .revision,
        3
    );
    assert!(DeleteRequest::new(&signer(), 0).is_err());
}

#[test]
fn signed_records_preserve_owned_relay_identity_and_ipv4_ipv6_socket() {
    use rds_discovery::OwnedRelayRoute;
    let key = EndpointKey(SigningKey::from_bytes(&[91; 32]).verifying_key().to_bytes());
    for socket in ["127.0.0.1:3340", "[::1]:3340"] {
        let route = OwnedRelayRoute {
            key,
            addr: socket.parse().unwrap(),
        };
        let uri = route.to_string();
        assert_eq!(uri.parse::<OwnedRelayRoute>().unwrap(), route);
        let mut issuer = RecordIssuer::memory(signer());
        let mut candidate = draft(4000);
        candidate.relay_urls.push(uri.clone());
        let record = issuer.record(candidate, now_unix().unwrap()).unwrap();
        let store = MemoryStore::default();
        store.put(&record).unwrap();
        assert_eq!(
            store
                .get(&issuer.key())
                .unwrap()
                .verify_fresh()
                .unwrap()
                .relay_urls,
            vec![uri]
        );
    }
}

#[test]
fn owned_relay_locator_rejects_ambiguous_fields_and_nonliteral_hosts() {
    use rds_discovery::OwnedRelayRoute;
    let route = OwnedRelayRoute {
        key: EndpointKey(signer().verifying_key().to_bytes()),
        addr: "127.0.0.1:3340".parse().unwrap(),
    }
    .to_string();
    for bad in [
        route.replace('@', ":password@"),
        format!("{route}?extra=1"),
        format!("{route}#fragment"),
        format!("{route}/path"),
        route.replace("127.0.0.1", "relay.example"),
        route.replace("127.0.0.1", "0.0.0.0"),
        route.replace(":3340", ":0"),
        "rds-relay://not-an-identity@127.0.0.1:3340".into(),
    ] {
        assert!(bad.parse::<OwnedRelayRoute>().is_err(), "{bad}");
    }
}

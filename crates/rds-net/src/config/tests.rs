use super::*;

#[test]
fn strict_schema_bounds_versions_and_roundtrip() {
    for invalid in [
        r#"{}"#,
        r#"{"schema_version":0}"#,
        r#"{"schema_version":2}"#,
        r#"{"schema_version":1,"typo":true}"#,
        r#"{"schema_version":1,"schema_version":1}"#,
        r#"{"schema_version":1,"relay":{"mode":"disabled","urls":[]}}"#,
        r#"{"schema_version":1,"relay":{"mode":"unknown"}}"#,
        r#"{"schema_version":1}{}"#,
    ] {
        assert!(
            EndpointSettings::from_json(invalid.as_bytes()).is_err(),
            "{invalid}"
        );
    }
    assert!(EndpointSettings::from_json(&vec![b' '; MAX_CONFIG_BYTES + 1]).is_err());
    let original = EndpointSettings::default();
    let parsed = EndpointSettings::from_json(&serde_json::to_vec(&original).unwrap()).unwrap();
    assert_eq!(parsed, original);
    parsed.into_endpoint().unwrap();
}

#[test]
fn explicit_flags_replace_file_lists_without_resetting_other_settings() {
    let from_file = EndpointSettings::from_json(br#"{"schema_version":1,"bind_addrs":["127.0.0.1:1234"],"relay":{"mode":"iroh","urls":["https://old.example/"]},"max_multipath_paths":3}"#).unwrap();
    let unchanged = from_file
        .clone()
        .apply(EndpointOverrides::default())
        .unwrap();
    assert_eq!(from_file, unchanged);
    let replaced = from_file
        .clone()
        .apply(EndpointOverrides {
            relays: vec!["https://new.example/".into()],
            ..Default::default()
        })
        .unwrap()
        .into_endpoint()
        .unwrap();
    assert_eq!(replaced.relays.len(), 1);
    assert_eq!(replaced.relays[0].host_str(), Some("new.example"));
    assert_eq!(replaced.bind_addrs, from_file.bind_addrs);
    assert_eq!(replaced.max_multipath_paths, Some(3));
    assert!(!replaced.discovery);
    let disabled = from_file
        .apply(EndpointOverrides {
            no_relay: true,
            ..Default::default()
        })
        .unwrap()
        .into_endpoint()
        .unwrap();
    assert!(disabled.relays.is_empty());
    assert!(!disabled.discovery);
    assert!(
        EndpointSettings::default()
            .apply(EndpointOverrides {
                no_relay: true,
                relays: vec!["https://relay.example/".into()],
                ..Default::default()
            })
            .is_err()
    );
}

#[test]
fn runtime_configuration_bounds_are_enforced() {
    let invalid = [
        EndpointConfig {
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap(); 2],
            ..Default::default()
        },
        EndpointConfig {
            max_multipath_paths: Some(0),
            ..Default::default()
        },
        EndpointConfig {
            max_multipath_paths: Some(33),
            ..Default::default()
        },
        EndpointConfig {
            alpns: vec![],
            ..Default::default()
        },
        EndpointConfig {
            alpns: vec![vec![]],
            ..Default::default()
        },
        EndpointConfig {
            alpns: vec![vec![1; 256]],
            ..Default::default()
        },
        EndpointConfig {
            alpns: vec![b"rds/0".to_vec(); 2],
            ..Default::default()
        },
        EndpointConfig {
            relays: vec!["https://relay.example/".parse().unwrap(); 9],
            ..Default::default()
        },
    ];
    for config in invalid {
        assert!(config.validate().is_err());
    }
    for url in [
        "file:///tmp/relay",
        "ftp://relay.example/",
        "https://name:password@relay.example/",
        "https://relay.example/path",
        "https://relay.example/?secret=value",
        "https://relay.example/#frag",
        "http://relay.example:0/",
    ] {
        assert!(
            EndpointConfig::default()
                .with_relay(url)
                .unwrap()
                .validate()
                .is_err(),
            "{url}"
        );
    }
    EndpointConfig {
        max_multipath_paths: Some(32),
        alpns: vec![vec![1; 255]],
        ..Default::default()
    }
    .validate()
    .unwrap();
}

#[test]
fn shipped_examples_are_versioned_and_match_the_build() {
    for body in [
        include_bytes!("../../../../examples/endpoint-direct.json").as_slice(),
        include_bytes!("../../../../examples/endpoint-iroh.json").as_slice(),
    ] {
        let settings = EndpointSettings::from_json(body).unwrap();
        let roundtrip =
            EndpointSettings::from_json(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(roundtrip, settings);
        settings.into_endpoint().unwrap();
    }
    let owned =
        EndpointSettings::from_json(include_bytes!("../../../../examples/endpoint-owned.json"));
    check_owned_example(owned);
}

#[cfg(feature = "transport-noq")]
fn check_owned_example(settings: Result<EndpointSettings, ConfigError>) {
    let settings = settings.unwrap();
    let bytes = serde_json::to_vec(&settings).unwrap();
    assert_eq!(EndpointSettings::from_json(&bytes).unwrap(), settings);
    let config = settings.into_endpoint().unwrap();
    assert_eq!(config.backend, Backend::Noq);
    assert!(config.relay_endpoint.is_some());
    assert!(config.relays.is_empty());
    assert!(!config.discovery);
}

#[cfg(not(feature = "transport-noq"))]
fn check_owned_example(settings: Result<EndpointSettings, ConfigError>) {
    assert!(settings.is_err(), "unavailable backend must be rejected");
}

#[cfg(feature = "transport-noq")]
#[test]
fn owned_relay_and_endpoint_identity_are_not_interchangeable() {
    EndpointConfig {
        backend: Backend::Noq,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap(); 2],
        ..Default::default()
    }
    .validate()
    .unwrap();
    assert!(
        EndpointConfig {
            backend: Backend::Noq,
            bind_addrs: vec!["127.0.0.1:2222".parse().unwrap(); 2],
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        EndpointConfig {
            backend: Backend::Noq,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap(); 17],
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    let settings =
        EndpointSettings::from_json(include_bytes!("../../../../examples/endpoint-owned.json"))
            .unwrap();
    let config = settings.clone().into_endpoint().unwrap();
    assert!(
        config.secret_key.is_none(),
        "relay public identity must never supply endpoint secret"
    );
    let changed = settings
        .apply(EndpointOverrides {
            backend: Some(Backend::Iroh),
            ..Default::default()
        })
        .unwrap();
    assert!(
        changed.into_endpoint().is_err(),
        "a backend override must not erase incompatible relay settings"
    );
    for addresses in [
        vec![],
        vec![crate::TransportAddr::Ip("0.0.0.0:12".parse().unwrap())],
        vec![crate::TransportAddr::Ip("127.0.0.1:0".parse().unwrap())],
        vec![
            crate::TransportAddr::Ip("127.0.0.1:1".parse().unwrap()),
            crate::TransportAddr::Ip("127.0.0.1:2".parse().unwrap()),
        ],
    ] {
        let mut invalid = config.clone();
        invalid.relay_endpoint.as_mut().unwrap().addrs = addresses.into_iter().collect();
        assert!(invalid.validate().is_err());
    }
}

# Iroh public-data CodeQL review — 2026-10-06

PR110 introduces vendored Iroh1.3 source to the repository scanner. The eleven
new Rust cleartext alerts are exact source/sink cases below, reviewed without
changing query coverage, disabling CodeQL or altering runtime cryptography.

`iroh-base1.3 SecretKey::public` derives the Ed25519 verifying key through
`SigningKey::verifying_key().to_bytes`; it never returns the signing seed. The
example/test logging sinks identified by alerts61–70 print that public EndpointId.
They do not print SecretKey::to_bytes or a credential. Those upstream demo/test
identifiers are intended connection identities, not deployment inventory.

Alert71 ends at PkarrRelayClient::publish.put(url). The appended URL segment is
SignedPacket::public_key().to_z32(), read from the first32serialized public-key
bytes. `iroh-dns1.3 SignedPacket` layout is publickey32/signature64/timestamp8/DNS;
its relay payload omits the public-key prefix. Signing uses a secret internally,
but that secret is not serialized or placed in the URL. The RDS adapter uses
Minimal/no public pkarr lookup for explicitly configured private relays.
Public discovery is an explicit different preset, not a secret-key export.

These exact alerts are false positives for secret-key disclosure because the
analyzer propagates secret-key taint through derived public identity objects.
This assessment does not deem arbitrary DNS/TXT content public, nor sanitize
other logging/transmission sinks. Only the11reviewed alert instances are in scope;
future actual credential flows remain subject to the unchanged security rules.
The exact published crypto/discovery implementation remains unchanged.

# Checkpoint C8 — 20260922-113544

Verdict: **pending review**

## Automated checks
- fmt/clippy/test: PASS (default + transport-noq lanes)
- systemd units: ProtectSystem=strict, NoNewPrivileges, PrivateTmp,
  empty CapabilityBoundingSet, AF_INET/6/UNIX/NETLINK (netlink for
  netwatch), @system-service filter, UMask=0077, StateDirectory-scoped
  writes: verified in both units
- key permissions: endpoint.key written 0600 by load_or_create_key;
  enforced at create; review checks stat %a on deployed hosts
- ports/firewall/runbook/failure-modes: docs/deployment.md
- release build + --version smoke: PASS
- REAL-METAL EVIDENCE: attested in the private estate repository's
  docs/reports/rds-e2e-<date>.md (host facts are estate-private) —
  this gate covers the artifact layer; ssh-across-NAT, soak and
  desktop-smoke rows are attested there

## Manual checklist (fill before merge)
- [x] All "not run" items above explained — every check ran; the
  real-metal rows are attested in the estate repo's
  docs/reports/rds-e2e-20260922.md (private host facts live there)
- [x] Reports committed: this file; bench reports belong to earlier
  waves — C8 evidence is the deployment itself
- [x] docs/ updated: deployment.md runbook + port/firewall tables +
  one-key-one-role note; architecture.md unchanged (WS8 adds no
  protocol change)
- [x] security/unsafe review done: no new unsafe; sandbox directives
  verified by the gate; AF_NETLINK added because netwatch needs it

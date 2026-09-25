# Engineering preview releases

The `0.1.0` release is an engineering preview for isolated evaluation. It is
not a production-readiness gate, a completed remote-desktop product, or an
automatic upgrade of development deployments. The detailed remaining work is
in [the remediation plan](remediation-plan.md). No consumer source pin or running
service changes automatically when a release is published.

## Contents and supported build hosts

| Asset | Contents / build host |
|---|---|
| `remote-device-sync-0.1.0-x86_64-unknown-linux-gnu.tar.gz` | Native `rds`, `rds-agent`, `rds-relay`, `rds-server`; Ubuntu 24.04 x86_64 |
| `remote-device-sync-0.1.0-aarch64-apple-darwin.tar.gz` | Same four native programs; macOS 15 arm64 |
| `remote-device-sync-0.1.0-source.tar.gz` | Tracked source and shared test fixtures, lockfile, docs, deployment and `ops/observability` configuration |
| `source-sbom.spdx.json` | Syft SPDX inventory of the **source archive**; not an OS/runtime or complete binary SBOM |
| `release-notes.md` | Compatibility and readiness notes |
| `release-manifest.json` | Source commit/tag object, target list, asset sizes and SHA-256 digests |
| `SHA256SUMS` | Digests of the other six assets |

Each native archive includes `build-info.json`, `Cargo.lock` and `LICENSE`.
Build info binds compiler, source commit, target, features and each binary's
SHA-256. Rust is pinned to `1.98.1`; Cargo uses `--locked`. The build enables
`rds-agent/transport-noq`, `rds-cli/transport-noq`, `rds-relay/owned-relay` and
`rds-server/owned-relay`, alongside the default transport. Desktop features are
excluded. All four installed programs are Rust executables; Python, Syft and
GitHub CLI are build/verification tooling, not runtime requirements.

Linux uses the build host's GNU libc; older distributions are not qualified.
macOS binaries are not Apple Developer ID signed/notarized or distributed as an
installer. Older macOS releases, Windows and other architectures are not
qualified. Archive determinism is tested; bit-for-bit reproducibility of Rust
binaries across independent hosts is not claimed.

## Download and verify before use

Use a new, empty working directory. These commands require GitHub CLI for
verification, not for running RDS. Choose exactly the target that matches the
host; the example is Linux:

```sh
version=0.1.0
repo=NDDev-OpenNetwork/remote-device-sync
target=x86_64-unknown-linux-gnu
# For an Apple Silicon Mac: target=aarch64-apple-darwin
gh release download "$version" --repo "$repo"
gh attestation verify SHA256SUMS --repo "$repo" \
  --signer-workflow "$repo/.github/workflows/release.yml" \
  --source-ref "refs/tags/$version" --deny-self-hosted-runners
# Linux:
sha256sum -c SHA256SUMS
# macOS equivalent: shasum -a 256 -c SHA256SUMS
archive="remote-device-sync-$version-$target.tar.gz"
gh attestation verify "$archive" --repo "$repo" \
  --signer-workflow "$repo/.github/workflows/release.yml" \
  --source-ref "refs/tags/$version" --deny-self-hosted-runners
tar -xzf "$archive"
"./rds-$version-$target/rds" --version
"./rds-$version-$target/rds" --help
```

The attestation source commit must match `release-manifest.json` and the
archive's `build-info.json`. An operator with an independently approved commit
can additionally pass `--source-digest <40-hex-commit>` to verification. A hash
file by itself does not authenticate a download: verify its attestation first.
The workflow verifies the published bytes and provenance after upload as well.

Evaluate from the extracted directory before integrating with a service
manager. Read [SSH](ssh.md), [shared sessions](local-sessions.md),
[grant leases](grant-leases.md), [policy state](policy-state.md),
[record state](record-state.md) and [observability](observability.md). Use the
current `--help` for each binary. Existing running agents are not replaced by
these instructions.

## Compatibility and remaining acceptance gates

This is the first published workspace release, but it breaks earlier
unreleased development contracts: grant v2, local IPC v3, signed policy/name
formats and durable directory format 3. Old peers may refuse new scopes and
wire messages. A supported automatic old-state migration does not yet exist.
Do not point preview executables at an existing deployment's state directory;
retain old keys/data and evaluate with explicit, separate enrollment and state.

Native SSH, manager/session selection and single-file synchronization have
local and CI coverage. Automated GDS grant issuance/renewal, complete managed
viewer/sync UX, interactive desktop rendering, native macOS/Wayland desktop
backends and input seat/key ownership remain open. Tests using synthetic
capture/input do not qualify a physical desktop. LAN/WAN/NAT, mixed-load,
physical-device, update/rollback and clean-machine platform qualification still
require W0–W10 evidence. Cloudflare/WARP is not a runtime dependency or a
qualified fallback in this release.

## Maintainer publication contract

`release.yml` builds and assembles identical asset types on pull requests and
`main` without publication rights. The tag path additionally requires:

1. An annotated numeric tag matching `VERSION`, workspace/lockfile versions,
   changelog and clean checked-out source; sign the tag when creating it.
2. That exact source commit on `main`, with successful default-branch `ci`,
   `supply-chain` and `codeql` runs. No inherited evidence from a predecessor.
3. Successful native builds on both hosts, executable version smoke checks,
   exact archive membership, source reconstruction, digests and SPDX validation.
4. The `release` environment's configured protections, official artifact
   attestations, and absence of an existing release. Network/auth errors cannot
   be interpreted as absence.
5. One creation containing all seven assets with `prerelease=true` and
   `latest=false`; existing release assets are never overwritten. Download,
   inventory, checksum and attestation verification follow publication.

A failure is fixed forward before tagging. If upload/publication fails, inspect
whether a draft or published release exists before retrying; never delete or
replace a published version to get a green run. The source SPDX attestation
applies only to the source archive. Provenance proves workflow origin; no SLSA
level or Apple code-signature claim is made.

The release workflow's build composition is module-owned because the shared
source-only publisher cannot attach native artifacts before publication. Shared
Rust/CodeQL/supply-chain workflows remain pinned. Standard official Actions
provide artifact transport/attestation; Syft is checksum/size/version pinned.
The Python stdlib helper only assembles and validates build artifacts; it has
no network or publication authority.

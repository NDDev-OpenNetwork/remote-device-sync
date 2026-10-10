# Multiple monitors and concurrent desktop sessions

One RDS connection may own multiple isolated desktop sessions. Each session
has a random route ID and its own control stream, frame route, decoder,
clipboard worker and display selection. The local manager already bounds the
number of sessions at 32 and the remote agent bounds connections and service
streams; one session ending cannot cancel another session on the same QUIC
connection.

Legacy display IDs retain X-screen numbering: display zero is the whole root
and still works with pre-RandR callers. Physical RandR 1.5 monitors have IDs in
the high-bit namespace, derived from X-screen number and monitor name. Hash
collisions are refused. Root and monitor entries come from one bounded catalog
shared by capabilities, capture and XTEST input; pointer offsets use checked
arithmetic. `--all-displays` prefers monitor entries when available.

Capture and input subscribe to RandR topology notifications and verify a
selected logical monitor's rectangle before native I/O. The additional geometry
reply covers monitor edits that do not emit an event on some X servers. If the selected
monitor disappears or its rectangle changes, that session refuses capture/input
and is reopened against fresh capabilities. It does not substitute a monitor
by list order. These IDs identify logical RandR monitors, not EDID hardware or
an immutable physical connector across arbitrary server configuration changes.

The viewer selects a display with `--display ID` (IDs come from `--list-displays`). It validates that the remote
capability list contains that index before forwarding input and uses
the selected extent for coordinate mapping. Running two viewers or two
managed channels for different display indices is supported; they must use
distinct per-session IDs. A duplicate route claim is refused locally before
any remote session work starts.

The standalone native application also accepts multiple targets and
`--all-displays`:

```sh
rds-viewer device-a device-b --resolution full-hd
rds-viewer device-a --list-displays
rds-viewer device-a --all-displays --max-fps 30
```

The default launch opens one [tabbed workspace](native-viewer.md#tabbed-workspace)
with at most eight display tabs across the requested devices. **Connect** and
**Open all displays** add actual inventories within that bound; selection and
settings are per tab. **Save workspace** stores tab order and profiles in
version-1 `viewer-workspace.json`; legacy `viewer.json` connections are imported
only when no saved or explicit workspace supersedes them. Explicit command-line
targets replace saved launch intent. The collapse arrow or Ctrl+Shift+H hides
both local bars while tab shortcuts remain available.

`--separate-windows` opts into at most eight independent child windows, each
with explicit peer/display/quality/clipboard choices. Config lists cannot
recursively launch more windows. Legacy version-1 `viewer.json` may contain
`connections` with `target`, `displays` and a device-specific `grant_file`. Grants
remain destination-bound and each device needs its own authorization; sharing
a workspace or launch does not widen a grant.

The protocol keeps control traffic at the highest QUIC priority and puts each
session's media on its own tagged unidirectional route. Clipboard chunks are interleaved with input/heartbeat work; independent
queues and budgets keep session ownership explicit.

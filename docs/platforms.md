# Platform matrix and backend selection

Supported targets: **Linux x86_64 (Ubuntu)** and **macOS arm64**. Both
must build green on every commit (CI matrix: `ubuntu-latest`,
`macos-latest`). Windows joins later behind the same backend seams —
nothing in the design blocks it.

The capture/codec/input preference tables below include planned backends.
Currently usable implementations are Linux X11 capture/XTEST input and the
OpenH264 software codec. The native wgpu viewer is implemented for Linux and macOS;
macOS capture/input, Wayland capture/input and hardware codecs remain stubs.
Build support is distinct from serving capability and native acceptance; see
[the capability matrix](capability-matrix.md).

## Selection model

Desktop platform capabilities use the `rds-desktop` traits
`Capturer`, `Encoder`, `Decoder` and `InputSink`. Audio currently exposes only
the [packet, codec and reorder core](audio.md); device source/sink interfaces
and adapters remain planned. Desktop backends expose a `probe`/`available`
function; a fixed preference order per platform picks the first usable
one at runtime — never compile-time alone. Probing logs the demotion
reason so "silently on the slow path" is impossible.

The orders below are the intended probe hierarchy. Only X11 capture/XTEST
input, OpenH264 software codecs and the native wgpu viewer are implemented.
Unavailable probes return no backend; the listed future protocols and drivers
do not imply supported hardware, consent or unattended access. Consult the
[capability matrix](capability-matrix.md) for current state.

## Capture order

| Linux x86_64 | macOS arm64 |
| --- | --- |
| 1. `ext-image-copy-capture-v1` — planned compositor protocol adapter, dmabuf + damage; compositor policy applies | 1. `ScreenCaptureKit` — display streams, IOSurface → VideoToolbox |
| 2. DRM/KMS scanout (`drm`+`gbm`) — unattended, headless, login screen; planned; device/seat/DRM-master access required | |
| 3. XDG portal + PipeWire — consented sessions, dmabuf-first, persist tokens for re-use | |
| 4. X11 RandR + `GetImage`/MIT-SHM — compatibility baseline (implemented) | |

## Codec order

| Linux | macOS |
| --- | --- |
| 1. Vulkan Video (`ash`) — H.264 enc/dec, planned adapter; GPU/driver support must be probed | 1. VideoToolbox — H.264/HEVC hw |
| 2. VA-API (`libva`) — Intel/AMD where Vulkan Video gaps | |
| 3. V4L2 mem2mem — embedded/ARM | |
| 4. OpenH264 (sw, implemented); AV1 remains planned | 2. OpenH264 sw floor |

H.264 constrained-baseline is the mandatory interoperable codec; HEVC/AV1
remain future codecs; their negotiation and backend acceptance are not implemented.

## Input order

| Linux | macOS |
| --- | --- |
| 1. Portal `RemoteDesktop` → EIS (`reis`, pure-Rust libei) | 1. `CGEvent` (accessibility TCC) |
| 2. wlroots virtual-kb/pointer | |
| 3. `/dev/uinput` — privileged, unattended/headless | |
| 4. XTEST (implemented) | |

## Render / audio

- Render: the optional native viewer implements a `wgpu` surface with a
  `winit` window (Metal on macOS, supported GPU backends on Linux), uploading
  software-decoded BGRA and presenting the newest pending image. Vulkan Video
  texture decode and Linux console DRM direct presentation remain planned.
- Audio has a bounded libopus packet/codec and jitter core. PipeWire/CoreAudio
  device I/O and agent/viewer playout remain unwritten; no audio service is
  advertised from that library alone.

## cfg conventions

The native [SSH client](ssh.md) shares safe rustix termios/readiness adapters
between Linux and macOS. Linux fixtures exercise a real OS PTY, terminal and
descriptor restoration, signals, resize and installed OpenSSH interoperability.
Native macOS execution and TUI/physical-network qualification remain pending.
The SSH backend uses the existing ring crypto/native boundary; no external
client/helper executable is part of the RDS command implementation.

The default local session manager uses filesystem Unix sockets and Tokio
`peer_cred()` on both supported targets, without custom unsafe code. Both ends
check effective UID. Directory/socket ownership and mode checks use safe rustix
and standard-library APIs. Paths reject symlink components; macOS callers must
use canonical paths. Linux integration evidence is recorded separately from
the still-required native macOS and distinct-user qualification.

Policy, capability-grant and endpoint-record leases use safe `rustix::time::clock_gettime`: `CLOCK_BOOTTIME` on Linux
and `CLOCK_MONOTONIC` on Darwin, both including suspend. These clocks have
different semantics across platforms; do not substitute Rust `Instant` for the
persisted lease deadline. References: [Linux clock documentation](https://man7.org/linux/man-pages/man2/clock_gettime.2.html),
[Apple clock documentation](https://github.com/apple-oss-distributions/Libc/blob/main/gen/clock_gettime.3)
and [Apple implementation](https://github.com/apple-oss-distributions/Libc/blob/main/gen/clock_gettime.c).

The Linux boot UUID is read from `/proc/sys/kernel/random/boot_id`; macOS uses
the read-only `kern.bootsessionuuid` sysctl ([XNU declaration](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sysctl.c)).
The macOS adapter contains one bounded `libc::sysctlbyname` call with a documented
unsafe block. This extends the platform FFI boundary for durable leases; no
external clock/OS command is invoked. Failure to obtain either clock or boot
identity closes policy admission. Native macOS and real suspend qualification
remain pending; Linux tests also inject boot changes and discontinuous clocks.

- Gate on `#[cfg(target_os = "...")]` for platform code, on features
  only for *optional* capability bundles (`x11`).
- Platform file naming: one backend per file under its function dir
  (`capture/kms.rs`, `input/portal.rs`, `codec/vulkan.rs`); no
  `#[cfg]` inside function bodies — gate the module declaration.
- `unsafe` is confined to backend files and flagged by lint policy;
  each unsafe block carries a `// SAFETY:` note.

## Native X11 test boundary

The required Linux CI Xvfb lane runs actual capture and input tests on two
isolated screens. Ordinary tests explicitly ignore those cases instead of
silently passing without DISPLAY or injecting into an ambient developer seat.
See [the X11 contract and reproduction command](x11-input.md). This fixture does
not qualify a composited desktop, RandR monitor changes, Wayland or macOS.

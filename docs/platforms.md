# Platform matrix and backend selection

Supported targets: **Linux x86_64 (Ubuntu)** and **macOS arm64**. Both
must build green on every commit (CI matrix: `ubuntu-latest`,
`macos-latest`). Windows joins later behind the same backend seams —
nothing in the design blocks it.

## Selection model

Every platform capability lives behind a trait in `rds-desktop`
(`Capturer`, `Encoder`, `Decoder`, `InputSink`) or `rds-audio`
(`AudioSource`/`AudioSink`). Backends expose a `probe`/`available`
function; a fixed preference order per platform picks the first usable
one at runtime — never compile-time alone. Probing logs the demotion
reason so "silently on the slow path" is impossible.

## Capture order

| Linux x86_64 | macOS arm64 |
| --- | --- |
| 1. `ext-image-copy-capture-v1` — privileged compositor protocol, dmabuf + damage, no consent dialog | 1. `ScreenCaptureKit` — display streams, IOSurface → VideoToolbox |
| 2. DRM/KMS scanout (`drm`+`gbm`) — unattended, headless, login screen; needs CAP_SYS_ADMIN or DRM master | |
| 3. XDG portal + PipeWire — consented sessions, dmabuf-first, persist tokens for re-use | |
| 4. X11 `GetImage` — compatibility baseline (implemented) | |

## Codec order

| Linux | macOS |
| --- | --- |
| 1. Vulkan Video (`ash`) — H.264 enc/dec, all GPU vendors, one implementation | 1. VideoToolbox — H.264/HEVC hw |
| 2. VA-API (`libva`) — Intel/AMD where Vulkan Video gaps | |
| 3. V4L2 mem2mem — embedded/ARM | |
| 4. OpenH264 (sw, implemented) → rav1e AV1 tier | 2. OpenH264 sw floor |

H.264 constrained-baseline is the mandatory interoperable codec; HEVC/AV1
negotiate only after both endpoints probe them.

## Input order

| Linux | macOS |
| --- | --- |
| 1. Portal `RemoteDesktop` → EIS (`reis`, pure-Rust libei) | 1. `CGEvent` (accessibility TCC) |
| 2. wlroots virtual-kb/pointer | |
| 3. `/dev/uinput` — privileged, unattended/headless | |
| 4. XTEST (implemented) | |

## Render / audio

- Render: `wgpu` surface on both OSes; Vulkan Video decode lands as
  `wgpu::Texture`; software path uploads BGRA. Linux console client
  option: DRM atomic direct present.
- Audio: PipeWire on Linux, `cpal`→CoreAudio on macOS; Opus (`opus`
  crate) both sides.

## cfg conventions

Policy leases use safe `rustix::time::clock_gettime`: `CLOCK_BOOTTIME` on Linux
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

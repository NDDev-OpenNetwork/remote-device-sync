# RDS application artwork

The master `app-icon.png` was generated with the built-in imagegen tool on
2026-09-30. It is a 1254×1254 RGBA image with actual transparent outer pixels.
The same artwork supplies:

- `app-icon.icns`: macOS bundle/Finder/Dock, with 16–1024 px representations.
- `app-icon-512.png`: Linux icon-theme installation.
- `app-icon-256.rgba`: 256×256 RGBA bytes embedded in native viewer windows.
- The embedded PNG: AppKit application icon for direct CLI viewer launches.
- `org.nddev.opennetwork.rds.desktop`: generic Linux desktop launcher, using the
  master PNG installed under its matching icon name.
- The repository README preview.

ICNS sizes are produced with macOS `sips` and `iconutil`; the raw derivative is
serialized from the 256 px PNG without altering the artwork. No image-decoder
crate is added to the viewer's hot path. Optional Pillow is needed only when
regenerating the raw authoring derivative, never for Rust builds or app launch.

The image contains no deployment or estate data. The workspace license applies.

## Generation prompt

Use case: logo-brand. Asset type: production application icon for RDS, a native
remote desktop and device sync app, displayed in macOS Dock/Finder and desktop
window/launcher surfaces. Create one polished square 1024×1024 raster icon. A
single distinctive, compact symbol of two connected desktop screens with a
subtle continuous connection/sync motif, on a softly rounded squircle tile.
Modern native desktop-app visual language, restrained dimensional depth, clean
silhouette, harmonious high-contrast color treatment, excellent legibility at
16–64 pixels. Center the symbol with balanced generous padding; keep every part
within the square and leave a narrow transparent outer margin around the tile.
Render an actual standalone icon with genuine alpha outside the tile. No text,
no letters, no screenshots, no surrounding scene, no extra badges, no branding
from other apps, no watermark. Avoid tiny details, busy circuitry, excessive
glow, or an icon sheet.

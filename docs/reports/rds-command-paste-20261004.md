# Native Command paste — 2026-10-04

W6.8 text-input qualification follow-up. The prior explicit clipboard path
recognized Ctrl+V only; macOS Command+V forwarded Super+V without reading the
local pasteboard. Native Command paste now reads text for that explicit gesture,
queues bounded clipboard chunks and a remote Control+V chord. Shift remains
held for terminal Control+Shift+V. The temporary chord restores original held
modifiers; Control brackets Super release/restore. Other physical shortcuts
retain their mapping. OS repeat and physical V release cannot repeat the local
paste side effect. Missing local text on Command paste sends no stale paste.

The chord regression verifies left/right Super, preserved Shift and an already
held right Control, exactly one V press, balanced ownership and original state
restoration. Expanded desktop units (99 tests), strict desktop/CLI clippy and
formatting passed. Native installed clipboard/target qualification remains
required; unit tests alone do not prove application paste or IME composition.

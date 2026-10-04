# Native button burst acceptance — 2026-10-04

W6.8 input qualification fixture. Checking only the final pointer button mask
cannot detect a lost intermediate rapid press. The isolated Xvfb regression
subscribes to native ButtonPress/ButtonRelease events, injects 100 immediate
press/release pairs alternating left/right/middle, and requires all 200 native
transitions in the exact order. It also requires every button released afterward.

This test runs only in the dedicated ignored-test Xvfb lane, never on an ambient
developer desktop. It uses the production XTEST sink, with no artificial hold
sleep between transitions. Its one-second final receive bound is a fixture
failure deadline, not an interactive performance target. Linux native execution
and strict clippy are being verified; macOS compilation excludes this Linux-only
fixture. Actual composited application handling and WAN input-to-visible-response
remain separate requirements.

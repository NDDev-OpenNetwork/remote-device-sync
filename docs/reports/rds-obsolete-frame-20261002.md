# Obsolete predecessors after recovery — 2026-10-02

Source review found two mechanisms capable of prolonging recovery. A predecessor
whose header arrives after a newer independent picture was classified as a
rejected reader and requested another key. Its stream stop was also treated as
failed delivery by the sender; a stop during payload writing ended the video
writer. Neither outcome describes a broken current reference when the receiver
has already recovered past that frame.

A real Iroh/Noq regression admits the older route, receives a newer key, then
supplies the old header. Old behavior fails because it requests another key.
New behavior releases that reader with a specific obsolete disposition, then
still requires repair for a malformed current header and accepts its replacement.

A second real-stream regression stops a bounded large payload while it is still
being written. Old behavior ends the writer. New behavior releases admission,
records disposal separately, preserves the connection and counts no fresh ACK
that could justify bitrate growth. An unknown stop code still selects failure;
only the exact obsolete code selects continuation. Both transport backends run
the scenarios, without a display or live user input.

The implementation uses an application STOP_SENDING code, without changing
serialized wire layouts or authority. Earlier senders retain their generic-stop
behavior. Reader/writer budgets, validation and deadlines are unchanged. Separate
obsolete counters prevent diagnostics from mislabeling disposal as delivery.
This advances W6 reference recovery, not a completed media acceptance checkpoint.

Installed native follow-up remains required. The predecessor's Full HD ten-minute
observation had decoded-age p95/max185/4079ms, and later ordinary-work logs retained
multi-second pauses despite no slow worker CPU record. Native dispatch-to-window
capture was also confounded by surface occlusion; it cannot establish a network
latency SLO. These negative results are retained and are not closed by regression
success. This defect explains a possible redundant repair path, not every freeze.

A related writer race also repeated repair: a receipt worker scheduled an IDR,
then its joined failure reset a chain that had already admitted the replacement
and scheduled another IDR. An equivalent-old-semantics regression fails on the
repeated request; corrected sequence-aware handling preserves the next delta
after independent recovery while still invalidating a failed current reference.
The latest produced-key marker coalesces repair for older frames; transport
success counters remain the sole successful-delivery growth signal.

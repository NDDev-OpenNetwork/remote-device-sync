# Passive control-reply read progress — 2026-10-06

W10/W6 diagnostic increment. A delayed reply's final RTT cannot distinguish a
pending prefix/body, a synchronous backend poll, a late runtime observer or a
downstream consumer wait. Sampling only completed replies also cannot show
progress during an active gap.

The desktop control reader now records bounded progress around its existing
framed read: four prefix bytes, expected body length, total bytes, poll count,
inside-poll flag and local progress time. A separate weak observer samples at
250 ms with skipped missed ticks, emitting at most once per read per second
when a newer outstanding probe or partial frame is overdue. Observer scheduling
lateness is explicit. No payload, key, clipboard contents or address is logged.

The reader retains its original wake-driven future: the observer never polls,
cancels or restarts it. This is important because periodically polling a read
could hide an underlying missed wake. No wire, timeout, retry, input replay,
path/bitrate default or application liveness policy changes. Progress locking
covers only a few local scalar updates and is never retained across I/O;
observer reads use try-lock and never query transport metadata.

An already confirmed newer probe prevents an older missing echo from falsely
classifying ordinary idle periods as stalled. Existing exact matching and
late-reply correlation remain. The observer does not own a connection or stream;
dropping the reader's observation guard aborts it, independently of critical
session task completion. It is not a lossless audit or a root-cause proof.

Tests use an actual partial framed duplex read, allowing observation ticks
between prefix/body fragments while preserving the final message. A never-waking
reader proves observation does not repoll its I/O future; owner drop stops
sampling. Probe tests preserve late correlation while excluding old missing
echoes from diagnostic freshness. Cross-platform qualification remains pending.

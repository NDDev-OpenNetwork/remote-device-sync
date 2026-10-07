# Persisted native viewer quality — 2026-10-07

`viewer.json` already supported a typed `resolution` value. Startup applied it,
then still opened the native picker because only the CLI value source was
checked. A subsequent picker response could overwrite the persisted choice.

Apply the CLI/file precedence before deciding whether a picker is needed.
An explicit CLI profile wins, a persisted profile bypasses the picker, and
headless execution never opens native UI. An unconfigured native launch keeps
the existing chooser. No protocol, permission or remote-process behavior changes.

The regression with the former picker condition failed for the intended reason.
Tests cover every persisted profile, an explicit CLI override, an unconfigured
native launch and headless execution. The public software CI remains required;
no native pixel or WAN acceptance is implied by configuration tests.

The existing [`ValueSource` API](https://docs.rs/clap/latest/clap/parser/enum.ValueSource.html)
distinguishes a defaulted argument from a command-line choice. The JSON choice
is an application source tracked separately; no new configuration mechanism is
introduced. The API distinction predates the requested research boundary;
this implementation observation is dated 2026-10-07.

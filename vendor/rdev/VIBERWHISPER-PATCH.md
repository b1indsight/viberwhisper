# ViberWhisper rdev patch

This directory contains the published **rdev 0.5.3** crates.io source snapshot,
including its MIT license. Upstream: https://github.com/Narsil/rdev.
Cargo registry bookkeeping (`.cargo-*` files) is omitted.
The original crate checksum is
`00552ca2dc2f93b84cd7b5581de49549411e4e41d89e1c691bcb93dc4be360c3`.

The macOS `common::convert` path sets `Event.name` to `None` instead of decoding
characters. Physical key events, mapping, event taps, Windows and Linux sources
are unchanged. ViberWhisper's desktop listener and setup helper only use `EventType`.
Decoding previously queried TIS/TSM from a background callback and triggered a
main-queue assertion on macOS 26. An unused upstream decoding helper is retained
with a local `dead_code` allowance; the public `KeyboardState` API is unchanged.

The added `viberwhisper_tests` regression converts local key events on a background
thread without installing a tap or posting keys. Run it explicitly:

```sh
cargo test --locked --manifest-path vendor/rdev/Cargo.toml --lib viberwhisper_tests
```

The published standalone lockfile is retained in version control for this check;
the local ignore file differs only by permitting that lockfile. macOS CI runs
the dependency-local regression explicitly, in addition to the application's suite.
Trailing whitespace in the upstream README template and bundled workflow is
normalized for repository diff checks; all other upstream files are unchanged.

On upgrades, compare against the original published source, recheck whether
upstream can disable listener character decoding, and retire this patch when
an upstream solution meets the same behavior. Do not edit the global Cargo cache.

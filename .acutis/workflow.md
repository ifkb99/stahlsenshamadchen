---
model: opus
effort: high
setup:
  - cargo fetch --locked
  - cargo build -p tactics_core --all-targets --locked --offline
gates:
  - name: determinism snapshot unchanged
    run: echo "eaf3a0e6b7c5f83dc6c47f3626fbbc9f8c825ae500443f15cb3d007ebfb1891a  crates/tactics_core/tests/snapshots/event_stream.txt" | sha256sum -c -
  - name: tactics_core tests
    run: cargo test -p tactics_core --locked --offline
    timeout_s: 1200
  - name: clippy (lib, bins, examples)
    run: cargo clippy -p tactics_core --lib --bins --examples --locked --offline -- -D warnings
  - cargo fmt --check
  - name: validate mods
    run: cargo run --locked --offline -p tactics_core --bin validate-mods
permissions:
  allow: ["Bash(cargo test:*)", "Bash(cargo build:*)", "Bash(cargo check:*)", "Bash(cargo clippy:*)", "Bash(cargo fmt:*)", "Bash(cargo run:*)"]
max_attempts: 2
permission_timeout_s: 1800
land: trunk
---
Ticket #{{ticket.id}}: {{ticket.title}}

{{ticket.body}}

About this repo:
- Read `CLAUDE.md` first and follow it. In particular: determinism given a seed, content is data, tests named as sentences, comments explain why.
- Work in `crates/tactics_core` (and `assets/mods/base` if the ticket needs data). The `game` crate (Bevy) needs system libraries that may be missing here, so don't build it.
- No network. Always pass `--offline`. The dependencies are already fetched and `tactics_core` is pre-built.
- Never run with `UPDATE_SNAPSHOTS=1`. A gate checks the determinism snapshot is byte-identical.
- `cargo test` prints one `test result:` line per test binary. Check all of them, not just the last.
- Measure with `--release` if you measure anything. The dev profile is `opt-level = 1`.

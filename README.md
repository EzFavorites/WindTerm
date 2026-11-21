# WindTerm

A cross-platform terminal emulator for the desktop, written in Rust and built on
[egui](https://github.com/emilk/egui). The terminal core is UI-agnostic: VT
semantics live in a pure crate with no I/O, no windowing, and no OS handles.

> **Status:** `v1.0.0`. Core, connection, and transfer paths are implemented and
> covered by an automated test suite (`cargo test --workspace` for the current
> count). A small number of heavyweight UI features are still being wired.

## Highlights

- **Terminal core** (`crates/term-core`): pure VT semantics — no UI, no I/O, no
  OS handles. Fully unit-testable and deterministic under test.
- **Native SSH** (`crates/term-ssh`) with host-key verification (TOFU /
  `known_hosts`), public-key, password and agent authentication, plus SSH
  tunnelling (`-L` / `-R` / `-D`) in `crates/term-tunnel`.
- **Session vault** (`crates/term-vault`): Argon2-based encrypted credential
  storage, kept as a leaf crate with no dependency on the terminal core.
- **Session library** persisted in SQLite (`crates/term-store`), with a
  migration path for MobaXterm session exports (`crates/term-import`).
- **SFTP** browsing and transfer (`crates/term-sftp`), file-transfer
  orchestration (`crates/term-xfer`), and **X / Y / ZMODEM** support
  (`crates/term-zmodem`).
- **Desktop UI** (`apps/desktop`): an egui front end with split panes, tabs,
  a file panel, a tunnel panel, a settings centre, key binding remapping, and
  a command palette.
- **Automation**: a Rhai scripting engine (`crates/term-script`), a JSON-RPC
  bridge (`crates/term-bridge`), and a headless CLI surface
  (`term-desk doctor` / `capability` / `serve` / `script run`).
- **Connection surface**: local shell (PTY), SSH, Telnet, Serial, plus
  jump hosts and per-session tunnels.

## Architecture

```
crates/
  term-core       pure VT and terminal semantics (no I/O, no UI)
  term-app        application/domain logic, menus, capability registration
  term-runtime    compatibility facade: re-exports of the old import paths
  term-session    session control plane
  term-script     Rhai scripting engine
  term-bridge     JSON-RPC bridge
  term-ssh        native SSH client (russh)
  term-tunnel     SSH tunnelling (-L / -R / -D)
  term-sftp       SFTP client
  term-xfer       file-transfer orchestration
  term-zmodem     X / Y / ZMODEM
  term-vault      encrypted credential vault
  term-store      SQLite session library
  term-import     session import (MobaXterm exports)
  term-io         session I/O and threads: PTY / serial / Telnet channels,
                  per-session supervisor, snapshot mailbox, session logging
  term-paths      path rules and data directories
  term-profile    user profile: preferences, layout, quick bar
apps/
  desktop         desktop binary (`term-desk`, egui UI)
```

Dependency direction is strictly one-way: `apps/desktop → term-app → term-core`,
with side effects concentrated in a few leaf crates (`term-io`, `term-ssh`,
`term-tunnel`, `term-sftp`). `tokio` is confined to those three network crates;
the desktop UI performs zero `.await`. `term-runtime` is a compatibility facade
over the old import paths only — new code depends on the implementation crates
directly, and the dependency direction is enforced by the build gates.

## Installing

Prebuilt packages are attached to each tagged release:

| Platform | File | Notes |
|---|---|---|
| macOS (Apple silicon) | `WindTerm-<ver>-macos-aarch64.tar.gz` | `.app` bundle |
| macOS (Intel) | `WindTerm-<ver>-macos-x86_64.tar.gz` | `.app` bundle |
| Linux (x86_64) | `WindTerm-<ver>-linux-x86_64.tar.gz` | `windterm` + `.desktop` + icon |
| Windows (x86_64) | `WindTerm-<ver>-windows-x86_64.zip` | `WindTerm.exe` |

Every archive ships with a `.sha256` alongside it:

```sh
shasum -a 256 -c WindTerm-1.0.0-macos-aarch64.tar.gz.sha256
```

The macOS builds are ad-hoc signed, not notarized. Gatekeeper will refuse the
first launch: right-click → *Open*, or

```sh
xattr -dr com.apple.quarantine WindTerm.app
```

## Building

Requires Rust **1.94** or newer (see `rust-toolchain.toml`; CI pins exactly
1.94.0).

```sh
cargo check --workspace          # type-check everything
cargo test --workspace           # run the test suite
cargo build -p term-desk         # build the desktop app
```

To produce the same package the release workflow publishes:

```sh
cargo build -p term-desk --release --target aarch64-apple-darwin
bash scripts/package-release.sh aarch64-apple-darwin 1.0.0 \
    target/aarch64-apple-darwin/release/term-desk dist
```

## Documentation

- [`docs/architecture.md`](docs/architecture.md) — layering, crate boundaries,
  and the invariants the build enforces.
- [`docs/building.md`](docs/building.md) — toolchain, local build, packaging,
  CI and release.
- [`docs/user-guide.md`](docs/user-guide.md) — sessions, SSH, SFTP, tunnels,
  scripting.
- [`docs/security.md`](docs/security.md) — credential vault, host-key
  verification, and the network surface.
- [`docs/scripting.md`](docs/scripting.md) — the Rhai API and the JSON-RPC
  bridge.
- [`docs/design/README.md`](docs/design/README.md) — per-module design
  documents: sessions and connection, terminal core, selection/find/scrollback,
  tabs and layout, files and transfer, credentials, scripting and automation,
  tmux, import and onboarding, crash hardening, theme and visual baseline,
  capability surface and menus, UI shell, scope and requirements, persistence,
  session I/O and threads.
- [`docs/adr/README.md`](docs/adr/README.md) — architecture decision records,
  one decision per number, with superseded links.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — how to run the gates before sending a
  patch.

## License

Licensed under the **Apache License 2.0**. See [`LICENSE`](LICENSE).

Third-party product names mentioned in documentation or compatibility code
(MobaXterm, SecureCRT, etc.) are trademarks of their respective owners. This
project is not affiliated with or endorsed by them.
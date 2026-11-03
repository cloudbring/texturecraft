# TextureCraft workspace tasks

`cargo xtask` retains TextureCraft's task list and adds shared syntax from `cli_help()`.
`craft-xtask` is a tooling-only dependency on the `xtask` crate, locked to `craft-libs`
integration commit `4acbbe2ff74d56fa0a2a9881094461ccce24943e`.

`src/config.rs` owns TextureCraft's layer assignments (`L0`–`L5`), exempt binaries/tooling,
and external crate rules (`egui`/`eframe`/`winit`/`rfd` restricted to `ui-egui` and apps;
`wgpu` restricted to `render`, `ui-egui` and apps), plus the local table/summary renderer
over `craft_xtask::layers::Report`. Version, ICO and ICNS dispatch directly to the shared
`craft_xtask::version` and `craft_xtask::ico` implementations.

`wasm` and `ci` orchestration stay app-owned in `src/main.rs`. TextureCraft has no `stats`,
`parity` or `pdf-deps` xtask subcommands; no unrelated tasks are introduced.

## Behavior boundaries

- Successful `layers`, `version`, `ico` and `icns` outputs and packed icon bytes reproduce
  the original golden fixtures byte-for-byte. Root `help` retains its original text and
  appends shared task syntax (`version::cli_help()`, `layers::cli_help()`, `ico::cli_help()`).
- `version tag` and `layers --metadata <FILE.json>` are now available through the shared
  argument parsers.
- Shared error diagnostics replace the old layer/version/icon error strings. Version
  lockfile refresh runs silently through `craft_xtask::Cargo::output` instead of echoing
  `$ cargo update --workspace --offline` to stderr.
- The shared layer metadata reader checks `workspace_members` and treats non-member path
  dependencies as external rather than workspace crates; this does not affect the current tree.

Only the `xtask` crate needs to be built or tested for this tooling migration:
`cargo test -p xtask`, `cargo clippy -p xtask --all-targets -- -D warnings`,
`cargo fmt --check -p xtask`, and `cargo xtask layers`.

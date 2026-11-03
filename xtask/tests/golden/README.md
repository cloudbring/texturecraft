# Ticket #70: original TextureCraft outputs

Captured on 2026-11-03 **before deleting the original implementations**, from pristine
TextureCraft HEAD `4edb8b8` in the supplied worktree. The complete fixture directory is
below 200 KB. No proprietary software, files or implementation sources were used. The PNG
hex fixtures encode original, programmatically generated transparent square images, not
product artwork or distributable app assets.

- `layers.stdout`, `help.stdout`, `version.stdout`: the original `xtask` binary on the
  current repository, built with `cargo build -p xtask` only.
- `metadata.json`: the same tree's `cargo metadata --format-version 1 --no-deps`, reduced
  to `workspace_members` and package IDs, names and dependencies, with checkout paths
  replaced by `/workspace`.
- `version-before.toml`: a dependency-free temporary workspace using the current version
  (`0.3.0`), edition (`2024`) and workspace inheritance. The original `version.rs` was
  compiled into a temporary harness whose Cargo cwd was that workspace. `version.stdout`,
  `version-set.stdout`, `version-after.toml` and `version-after.lock` capture a read and
  a real offline lockfile update to `1.2.3-rc.1`. The production workspace was not changed.
- `pngs.json`: PNG hex for transparent square images of sizes 16, 32, 64, 128 and 256 px;
  `ico.hex`, `ico.stdout`, `icns.hex` and `icns.stdout` capture the original CLI's packed
  bytes and stdout lines in the same input order.

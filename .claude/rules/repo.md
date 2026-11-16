# apfsmount: repo, build, CI

apfsmount mounts Mac file systems (APFS and HFS+) read-only on Windows through WinFsp.
Write support is not a feature.

## Layout
A Cargo workspace with one product.
- `products/apfsmount/` the product: `src/lib.rs` (`PRODUCT`: names, URLs, texts), `src/mac.rs` (the backend:
  finding APFS and HFS+ by magic), `src/apfs.rs` (APFS through apfs-core), `src/hfs.rs` (HFS+ through
  dpp's `hfsplus`; `cmpfs` decompresses), the two thin exes,
  `build.rs` (icon, manifest, link flags), `installer.iss` (defines), `logos/`, `THIRD-PARTY-NOTICES.txt`, `tests/`,
  `examples/walk.rs` (walks and times a file system without WinFsp), `examples/mkhfs.rs` (CI's HFS+ image).
- `apfs-core` is a git dependency on github.com/enesilhaydin/apfs-explorer, pinned to a release tag
  (GPL-3.0-only). Fixes go upstream as lean PRs from a fork.
- `hfsplus` and `cmpfs` come from our fork github.com/poeggi/dpp, pinned to a commit: it adds
  `catalog::list_directory_records` (Dil4rd/dpp#22). Once a dpp release has it, they go back to crates.io.
  A tag `apfsmount-hfsplus-<short rev>` in the fork keeps the pinned commit reachable; a new pin gets one too.
  dpp PRs target its `dev` branch and follow its CLAUDE.md (pinned toolchain, -dev versions, checks).
- `installer/` `common.iss` and `winfsp.iss`, copies from ufsmount at the pinned tag.
- All build output and other temporary files go to `temp/` (git-ignored; set in `.cargo/config.toml`).
  The setup lands in `temp/installer`. Nothing built is committed: releases carry the exes and the setup.

## Shared code: win-mountkit
- A git dependency on github.com/poeggi/ufsmount, pinned to a tag in the root `Cargo.toml`.
  It holds disk access, partition tables, the volume layer, WinFsp, launcher, `cp`, CLI, tray and About.
- Never patch it here. Changes are made, tested and released in the ufsmount repo, in a session there.
  Then the tag moves here: `Cargo.toml`, `cargo update -p win-mountkit`, and `installer/*.iss` from the new tag.
- A test checks that `Cargo.lock` is on the tag. CI checks that `installer/*.iss` equal the tag's files.
- A git dependency carries no link setup. This product supplies its own: `+crt-static` (`.cargo/config.toml`),
  the link args in `build.rs` (both `/DELAYLOAD`s, `/IGNORE:4199`, `/DEPENDENTLOADFLAG:0x800` for bins),
  the tray manifest (Common Controls 6), and `Product.winfsp_rs` from this Cargo.lock.
  New fields in `Product`, `Found` or `Info` break the build until they are set here.
- `/IGNORE:4199` also hides a `/DELAYLOAD` name that matches nothing. Then `winfsp-x64.dll` becomes a static
  import and every exe fails at start: the smoke job's `apfsmount.exe --version` catches that.
- Version 1.0.0 only when every upstream PR is merged, an Inno Setup release fixes the high-DPI checkboxes
  (the Offset workaround in `common.iss` is gone), and a newer WinFsp runs cleanly. Until then 0.x.
- The `Product` struct, the `Backend`/`Fs` traits and how the shared code behaves: see ufsmount's
  `crates/win-mountkit/src/product.rs`, `fs.rs` and its `.claude/rules/`. Read them there, by absolute path.

## Build and test
- Rust stable, MSVC target, VS Build Tools (C++), LLVM (libclang for bindgen in winfsp-sys).
- Before a commit: `cargo fmt --check`, `cargo clippy --all-targets --locked --no-deps -- -D warnings`, `cargo test --locked`.
- `winfsp-x64.dll` is delay-loaded by full path from the WinFsp InstallDir. `build.rs` sets the link flags
  for the exes and tests (exes also get `/DEPENDENTLOADFLAG:0x800`). `/IGNORE:4199` silences targets
  without imports from a delay-loaded DLL.
- The file system name Windows shows (`fs::Info.fs`: "APFS") comes from the backend, per volume.
- Every exe and test calls `apfsmount::init()` first.

## CI (`.github/workflows/ci.yml`)
- build: fmt, the installer scripts against the tag, clippy, tests, release build, the setup,
  and a 0.0.0 setup for the downgrade test.
- smoke: a fresh runner without WinFsp. Installs (downloads WinFsp), mounts the build's HFS+ image
  (`examples/mkhfs.rs`) on X: and reads it, starts the tray, downgrades and upgrades,
  uninstalls, and checks that nothing is left and PATH is unchanged.
- release: tags `v*` only. The tag must equal the version in the root `Cargo.toml` (`[workspace.package]`).
  See the release skill.
- Actions are pinned by commit SHA with the version as a comment. Tag builds run without the cache.
  Every job has a timeout.

## Conventions
- Releases only on the user's explicit request.
- ASCII only in every file. Each source file has the SPDX line `AGPL-3.0-or-later`.
- Dependencies: the latest stable release, checked upstream.
- `products/apfsmount/THIRD-PARTY-NOTICES.txt` is maintained by hand. When dependencies change, compare it with
  `cargo tree -p apfsmount -e normal --target x86_64-pc-windows-msvc --prefix none --format "{p}|{l}"`.
- Licenses: ours AGPL-3.0-or-later (win-mountkit too). apfs-core GPL-3.0-only, hfsplus and cmpfs MIT, winfsp-rs GPL-3.0,
  WinFsp GPL-3.0 with FLOSS exception.
  Dual-licensed crates are used under Apache-2.0.

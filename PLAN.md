# apfsmount: plan

Mount Mac disks (APFS and HFS+) read-only on Windows, the way ufsmount does for UFS2.
Repo poeggi/apfsmount. License AGPL-3.0-or-later.

## What it is
- `apfsmount.exe`: the CLI (probe, ls, cp, mount, register), as `ufsmount.exe`.
- `apfsmount-tray.exe`: the tray. It mounts every APFS or HFS+ volume it finds, at most 8.
- `apfsmount-<version>-setup.exe`: installs both, plus WinFsp when missing.
- It installs side by side with ufsmount. All system names come from the product id:
  task `apfsmount-tray`, `Global\apfsmount-tray`, `ProgramData\apfsmount`, launcher class `apfs`.
  It gets its own AppGuid.
- Read-only by design. Write support is not a feature.

## Shared code
The shared crate is `win-mountkit` from the ufsmount repo. It holds disk access, partition
tables, the volume layer, WinFsp, the launcher, `cp`, the CLI, the tray and the About dialog.
- Cargo git dependency, pinned to a tag:
  `win-mountkit = { git = "https://github.com/poeggi/ufsmount", tag = "vX.Y.Z" }`.
- The setup scripts `installer/common.iss` and `installer/winfsp.iss` are copied from the
  same tag. A CI step compares them with that tag, so they cannot drift apart.
- Changes to win-mountkit are made in the ufsmount repo, in a session there, and released
  there. apfsmount then moves its tag. Never patch the shared code inside apfsmount.
- Model the product on `products/ufsmount` in the ufsmount repo:
  - `src/lib.rs`: `PRODUCT` (id, fs, launcher class, first letter, repo, texts, credits,
    license, GPT types, backend) and `init()`.
  - The two thin exes, `build.rs` (icon, manifest, link flags), `installer.iss` (defines only).
  - `logos/`, `THIRD-PARTY-NOTICES.txt`, `tests/`.
  - `CLAUDE.md` and `.claude/rules/` adapted from ufsmount's. Most of security.md and
    installer.md apply as they are.

## Reader
Decided 2026-11-16: APFS is read with `apfs-core` from apfs-explorer (git dependency, tag v1.3.0).
dpp's `apfs` scans its B-trees from the first leaf on every lookup, too slow for a real disk.
HFS+ is read with dpp's `hfsplus` (MIT), from our fork until Dil4rd/dpp#22 is released.
apfsmount reads file data through its own extent map with the overflow extents.

- Both readers work through `Read + Seek`, so win-mountkit's `PartitionReader` plugs in.
- Not usable: apfs-fuse (GPL-2.0-only, C++), hfsfuse (C code in the elevated tray).
- Check every dependency's latest release upstream before adding it.

## Finding volumes
- GPT types: APFS `7c3457ef-0000-11aa-aa11-00306543ecac`, HFS+ `48465300-0000-11aa-aa11-00306543ecac`.
  APM entries `Apple_HFS`, `Apple_HFSX`; the whole disk when there is no table.
- Every candidate is checked by its own magic, never trusted by type:
  - APFS: container superblock `NXSB` at byte 32 of block 0. One container holds several
    volumes: report each one (win-mountkit gives them ids `p2v1`, `p2v2`, ...).
  - HFS+: `H+` or `HX` at byte 1024; an HFS wrapper (`BD`) with an embedded HFS+ volume.
- Volume roles: mount the user's volumes. Preboot, Recovery, VM and Update are listed but not
  mounted automatically (decision below).
- Encrypted volumes (FileVault) are found and refused, like UFS1 in ufsmount. Snapshots and
  Fusion drives are not read.

## Names and files
- macOS stores names decomposed (NFD). Show them composed (NFC); look up by either form.
- APFS volumes are usually case-insensitive, HFSX case-sensitive. win-mountkit's lookup is
  case-sensitive with a unique case-insensitive fallback. If a volume needs real
  case-insensitive lookups, that is a win-mountkit change (in the ufsmount repo).
- Transparent decompression (decmpfs, resource fork) through the reader.
- Symlinks as Windows symlinks; hard links as the same file; timestamps from APFS
  nanoseconds and HFS+ seconds since 1904. Extended attributes and resource forks are not shown.

## Untrusted disks
- The same caps and rules as ufsmount: bounds on directory sizes and entry counts, no panic on
  crafted images, a bad sector fails only the file that uses it.
- Fuzz the probe and the reader with damaged images. Findings in the reader crates go
  upstream as lean PRs (one fix per commit, no extra comments or cleanup).

## Milestones
1. Repo skeleton: product, exes, setup, CI (build, smoke, release) copied from ufsmount and
   renamed. It builds and installs with a stub backend.
2. HFS+ read with images: probe, list, read, `cp`, mount.
3. APFS read on the user's real APFS disk: one volume, then whole containers.
4. Names (NFC), case rules, compression, links, timestamps; encrypted volumes refused.
5. Hardening: crafted images, fuzzing, caps. Tests for every layout found.
6. Logo: check the icon in the real taskbar and at every DPI. Milestone 1 already renders
   it from the SVG sources (see Decisions). Any change needs a mockup and the user's go.
7. README, THIRD-PARTY-NOTICES (MIT notices of the reader crates), Fable review.
   Release 0.1.0 only when the user asks.

## Testing
- Real disks, read-only: an HFS+ Mac OS X system volume on a USB disk, and a USB stick
  formatted APFS by iPadOS (and HFS+ before that).
- Next: a real Mac disk (APFS system container, compression, symlinks, many files).
- CI smoke test: a fresh runner installs the setup, starts the tray, upgrades, uninstalls,
  and checks nothing is left. A mount of a test image in CI is still to come.

## Decisions (settled 2026-11-16)
- First drive letter: M:. ufsmount starts at U:.
- System volumes (Preboot, Recovery, VM, Update): listed, never mounted automatically.
- Logo: variant A, picked by the user. A stylized green apple behind the M: drive, in
  ufsmount's layout (its daemon behind the U: drive). A dark green leaf, no stem, no glare.
  A bite on the left, the mirror side of Apple's logo (the user's choice).
  Green, not red: it sets the icon apart from ufsmount's red daemon in the taskbar.
  Sources: `products/apfsmount/logos/logo.svg` and `logo-small.svg` (32 px).
  The icon and setup images are rendered from them, as in ufsmount.
- PLAN.md stays in the repo as the plan document.

## Rules carried over from ufsmount
- Releases only on the user's explicit request; a Fable review of unchecked changes first.
- ASCII only; SPDX `AGPL-3.0-or-later` in every source file; short, present-tense comments.
- UI changes (tray, About, setup, icons): mockup first.
- No "Co-Authored-By" trailers in commits.

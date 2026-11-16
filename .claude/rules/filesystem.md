# apfsmount: file system behaviour

- Read-only. The shared code returns `STATUS_MEDIA_WRITE_PROTECTED` for every writing callback.
- Volumes are found by their magic, never by the partition type: APFS `NXSB` at byte 32 of block 0;
  HFS+ `H+`, HFSX `HX` at byte 1024; an HFS wrapper `BD` with `H+` at byte 1024 + 0x7c. Plain HFS is refused.
- APFS is read through `apfs-core` (apfs-explorer, `src/apfs.rs`), by inode number.
  Tested on a real disk (2026-11-16: a USB stick formatted APFS by iPadOS, 1,363 files checked against `cp`).
- Both backends report `..` with the real parent in `read_dir` (HFS+: the thread record, APFS: the inode).
  win-mountkit takes a directory's parent only from there; without it `..` in symlink targets fails.
- HFS+ and HFSX are read through dpp's `hfsplus` (`src/hfs.rs`), by catalog node id. File data goes through
  our own extent map with the overflow extents (the crate's `ForkReader` follows only the 8 inline ones).
  Hard links (files and folders) show their target; the private link folders are hidden.
  hfsplus orders names unlike macOS (NUL, ignorable characters), so a lookup by name can miss:
  the private folders are found in the root's listing, and a missed lookup searches the parent's listing.
  Compressed files
  are decompressed whole, at most 1 GiB. An HFS wrapper is read at its embedded volume. The journal is
  not replayed. Names: composed (NFC), `/` shown as `:` like macOS. Plain HFS is refused.
  Tested on a real disk (2026-11-16: a Mac OS X system volume, 10,765 entries, 2 GB copied).
- An APFS container holds several volumes. win-mountkit gives them ids `<place>v1`, `<place>v2`, ...
  A volume's file system id is its UUID in hex.
- Encrypted volumes are found and refused. Snapshots and Fusion drives are not read.
- System volumes (Preboot, Recovery, VM, Update) are `Found.on_request`: listed, never mounted automatically.
  The tray mounts them from the menu; the command line and the launcher need their part named (`--part p2v3`).
- Names: macOS stores them decomposed (NFD); they show composed (NFC).
- A drive letter mount that fails while the letter is free is tried once more after 0.5 s (win-mountkit):
  WinFsp mounts of other processes (ufsmount's tray) starting at the same moment can make it fail.
- Untrusted disks: bounded sizes and counts, no panic on crafted images. A bad sector fails only the file
  that uses it. The shared code's caps apply (see ufsmount's security rules).

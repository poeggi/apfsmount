# apfsmount: security rules

Raw disk reads and global drive letters need admin rights. The tray and the launcher mode run elevated.

- The elevated code (tray, task, launcher, folders, update check) is win-mountkit's. Its rules are ufsmount's
  `.claude/rules/security.md` and apply here unchanged. Changes to it are made in the ufsmount repo.
- Product names (`id`, `launcher_class`) are `[a-z0-9][a-z0-9-]{0,31}`; `set_product` checks them.
  They reach `schtasks` command lines and registry paths unquoted.
- The launcher registration keeps WinFsp's default Security (SYSTEM and Administrators).
  Never grant Everyone: that exposes every raw disk.
- The backend reads untrusted disks inside the elevated tray: every size, offset and count read from a disk
  is checked before use, and a crafted disk never panics, loops or allocates without bound.
- No C code in the reader: it runs elevated.
- Each request (probe, open, stat, list, read, readlink) reads at most `READ_BUDGET` (512 MiB) in both
  backends, so a reader crate that loops over the disk fails the request. Not covered: apfs-core's own node
  cache (a fan-out over cached nodes) and its allocations sized by on-disk values; those fixes go upstream.

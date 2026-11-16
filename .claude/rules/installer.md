# apfsmount: setup, icon and tray

## Setup (Inno Setup 7.1.1)
- `products/apfsmount/installer.iss` holds the product's defines (name, AppGuid, repo, launcher class) and
  includes `installer/common.iss`. Relative paths in it are relative to the product's script.
  The AppGuid never changes: upgrades find the installed version by it.
- `installer/*.iss` are ufsmount's, unchanged. Setup behaviour is described in ufsmount's
  `.claude/rules/installer.md`; changes to it are made there.
- Build: `ISCC /DAppVersion=<version> products\apfsmount\installer.iss`. Run ISCC from PowerShell or cmd:
  Git Bash rewrites `/D` arguments.
- It installs side by side with ufsmount: every system name comes from the product id
  (task `apfsmount-tray`, `Global\apfsmount-tray`, `<ProgramData>\apfsmount`, launcher class `apfs`).
  Only WinFsp is shared.
- Before a WinFsp update, WinFsp drives still mounted after the tray stopped (ufsmount, command line
  mounts) are listed by WinFsp's `fsptool lsvol`, and the setup asks first. Silent installs with
  `/SUPPRESSMSGBOXES` go on. Cancel starts the stopped tray again.
- The setup is unsigned: SmartScreen warns, Smart App Control blocks it.

## Icon and setup images
- Sources: `products/apfsmount/logos/logo.svg` and `logo-small.svg` (the 32-unit drawing for small sizes).
  A green apple with a dark green leaf and a bite on the left, behind a white drive with black "M:".
  Green sets it apart from ufsmount's red daemon at taskbar sizes.
- `logos/apfsmount.ico` (exe, tray and setup icon): 16-40 px from `logo-small.svg` on its 32-unit grid;
  48 and 64 px from `logo-small.svg` with viewBox `-1.25 0.15 30.5 30.5`; 256 px from `logo.svg`.
- `logo.svg`'s viewBox ends at the drawing, so icons fill their canvas like other apps' icons.
- `logos/wizard/*.png` are rendered from `logo.svg` at the sizes the Inno Setup help lists per DPI:
  small ones fill the image, large ones hold a square of 70% of the width, centred at 38% of the height.
- Rendered with Inkscape. A changed logo needs a mockup and the user's go first.

## Tray
- The tray is win-mountkit's. Names and texts come from `PRODUCT`: "Mac" disks, first letter M:.
- Letters: M:, then L: down to D:, then N: up to U:. ufsmount starts at U: and goes down, so the two keep
  to their own letters (8 mounts each) unless other drives take the ones between.
  The letter menus ("Mount as", "Drive letter") list U: down to D:, as for every product.
- A file system marked `Found.on_request` (APFS system volumes) is always listed under "Not mounted" and
  mounts only from the menu; after a Rescan or a refused eject it is listed again.
- About's versions come from rustc and this repo's Cargo.lock through `build.rs`
  (`Product.winfsp_rs`, apfs-explorer's tag, win-mountkit's version).

; apfsmount setup (Inno Setup 7). Build: ISCC /DAppVersion=x.y.z products\apfsmount\installer.iss
; Expects temp\release\apfsmount.exe and temp\release\apfsmount-tray.exe.

#define AppName "apfsmount"
; Never change it: upgrades find the installed version by it.
#define AppGuid "C03C0B73-1D41-4456-8DB1-D8975EF96060"
#define AppPublisher "poeggi"
#define AppRepo "poeggi/apfsmount"
#define AppComment "Mount Mac disks (APFS and HFS+) read-only"
#define LauncherClass "apfs"

#include "..\..\installer\common.iss"

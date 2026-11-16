// SPDX-License-Identifier: AGPL-3.0-or-later

const TRAY_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;

/// The version of `name` in the workspace's Cargo.lock.
fn locked_version(lock: &str, name: &str) -> String {
    let key = format!("name = \"{name}\"");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line == key
            && let Some(v) = lines.next().and_then(|l| l.strip_prefix("version = \""))
        {
            return v.trim_end_matches('"').to_string();
        }
    }
    panic!("{name} is not in Cargo.lock");
}

/// The tag a git dependency from `repo` is pinned to in Cargo.lock.
fn locked_tag(lock: &str, repo: &str) -> String {
    let key = format!("source = \"git+{repo}?tag=");
    lock.lines()
        .find_map(|l| l.strip_prefix(key.as_str()))
        .and_then(|rest| rest.split('#').next())
        .unwrap_or_else(|| panic!("{repo} is not in Cargo.lock"))
        .to_string()
}

fn main() {
    // winfsp-x64.dll is loaded at runtime by full path from the WinFsp install
    // directory, so it must not be resolved at process start.
    println!("cargo:rustc-link-lib=dylib=delayimp");
    // All targets, the tests included.
    println!("cargo:rustc-link-arg=/DELAYLOAD:winfsp-x64.dll");
    // TaskDialogIndirect is in Common Controls 6 only, which the tray's
    // manifest selects; other targets would fail to start on its import.
    println!("cargo:rustc-link-arg=/DELAYLOAD:comctl32.dll");
    // LNK4199: a target that ends up with no imports from a delay-loaded DLL,
    // such as the CLI once LTO drops the tray code.
    println!("cargo:rustc-link-arg=/IGNORE:4199");
    // Static imports are searched in System32 only, never in the exe folder.
    println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=logos/apfsmount.ico");
    println!("cargo:rerun-if-changed=../../Cargo.lock");

    // The versions of win-mountkit, winfsp-rs and the APFS reader, for the About box.
    let lock = std::fs::read_to_string("../../Cargo.lock").expect("read Cargo.lock");
    let kit = locked_version(&lock, "win-mountkit");
    println!("cargo:rustc-env=APFSMOUNT_MOUNTKIT={kit}");
    let winfsp = locked_version(&lock, "winfsp");
    println!("cargo:rustc-env=APFSMOUNT_WINFSP_RS={winfsp}");
    let apfs = locked_tag(&lock, "https://github.com/enesilhaydin/apfs-explorer");
    println!("cargo:rustc-env=APFSMOUNT_APFS_EXPLORER={apfs}");
    let hfs = locked_version(&lock, "hfsplus");
    println!("cargo:rustc-env=APFSMOUNT_HFSPLUS={hfs}");

    // The logo as the icon (resource id 1, also used by the tray), plus version info.
    let mut cli = winresource::WindowsResource::new();
    cli.set_icon_with_id("logos/apfsmount.ico", "1").set(
        "FileDescription",
        "apfsmount - mount Mac disks (APFS and HFS+) read-only",
    );
    cli.compile_for("apfsmount")
        .expect("compile the resources of apfsmount");

    let mut tray = winresource::WindowsResource::new();
    tray.set_icon_with_id("logos/apfsmount.ico", "1")
        .set("FileDescription", "apfsmount tray")
        .set_manifest(TRAY_MANIFEST);
    tray.compile_for("apfsmount-tray")
        .expect("compile the resources of apfsmount-tray");
}

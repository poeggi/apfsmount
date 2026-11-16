// SPDX-License-Identifier: AGPL-3.0-or-later
//! apfsmount: Mac disks (APFS and HFS+) read-only on Windows. This crate holds
//! what is APFS, HFS+ or apfsmount; the rest is the shared win-mountkit.

mod apfs;
mod hfs;
mod mac;

pub use win_mountkit;
use win_mountkit::Product;

pub static PRODUCT: Product = Product {
    id: "apfsmount",
    fs: "Mac",
    fs_variant: "APFS or HFS+",
    launcher_class: "apfs",
    first_letter: 'M',
    repo: "poeggi/apfsmount",
    version: env!("CARGO_PKG_VERSION"),
    winfsp_rs: env!("APFSMOUNT_WINFSP_RS"),
    description: env!("CARGO_PKG_DESCRIPTION"),
    about: "Mounts Mac disks (APFS and HFS+) read-only.",
    built_using: concat!(
        "- apfs-core from APFS Explorer ",
        env!("APFSMOUNT_APFS_EXPLORER"),
        " by enesilhaydin\n- hfsplus ",
        env!("APFSMOUNT_HFSPLUS"),
        " from dpp by Dil4rd\n- win-mountkit ",
        env!("APFSMOUNT_MOUNTKIT"),
        ", from ufsmount"
    ),
    credits: "  apfs-core, from APFS Explorer by enesilhaydin and contributors (GPL-3.0-only)
    https://github.com/enesilhaydin/apfs-explorer
  hfsplus and cmpfs, from dpp by Dil4rd (MIT)
    https://github.com/Dil4rd/dpp
  win-mountkit, from ufsmount (AGPL-3.0-or-later)
    https://github.com/poeggi/ufsmount",
    license: "AGPL-3.0-or-later",
    quiet_log: "apfs_core",
    gpt_types: &[
        ("7c3457ef-0000-11aa-aa11-00306543ecac", "apple-apfs"),
        ("48465300-0000-11aa-aa11-00306543ecac", "apple-hfs"),
    ],
    backend: &mac::Mac,
};

/// Set this product for the shared code. Every exe and test calls it first.
pub fn init() {
    win_mountkit::set_product(&PRODUCT);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The setup's defines match the product.
    #[test]
    fn setup_defines() {
        let iss = include_str!("../installer.iss");
        let define = |name: &str| {
            let prefix = format!("#define {name} \"");
            iss.lines()
                .find_map(|l| l.strip_prefix(prefix.as_str()))
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or_else(|| panic!("{name} is not defined"))
        };
        assert_eq!(define("AppName"), PRODUCT.id);
        assert_eq!(define("AppRepo"), PRODUCT.repo);
        assert_eq!(define("LauncherClass"), PRODUCT.launcher_class);
        // The update check downloads the setup under this name.
        let common = include_str!("../../../installer/common.iss");
        assert!(common.contains("OutputBaseFilename={#AppName}-{#AppVersion}-setup"));
    }

    #[test]
    fn versions_come_from_the_build() {
        let lock = include_str!("../../../Cargo.lock").replace("\r\n", "\n");
        let kit = env!("APFSMOUNT_MOUNTKIT");
        assert!(lock.contains(&format!("name = \"win-mountkit\"\nversion = \"{kit}\"\n")));
        assert!(PRODUCT.built_using.contains(kit));
        let winfsp = PRODUCT.winfsp_rs;
        assert!(lock.contains(&format!("name = \"winfsp\"\nversion = \"{winfsp}\"\n")));
        let hfs = env!("APFSMOUNT_HFSPLUS");
        assert!(lock.contains(&format!("name = \"hfsplus\"\nversion = \"{hfs}\"\n")));
        assert!(PRODUCT.built_using.contains(hfs));
        let apfs = env!("APFSMOUNT_APFS_EXPLORER");
        assert!(lock.contains(&format!("apfs-explorer?tag={apfs}#")));
        assert!(PRODUCT.built_using.contains(apfs));
    }

    /// win-mountkit and the setup scripts come from the same ufsmount tag.
    #[test]
    fn shared_code_is_pinned_to_a_tag() {
        let toml = include_str!("../../../Cargo.toml");
        let lock = include_str!("../../../Cargo.lock");
        let tag = toml
            .lines()
            .find_map(|l| l.strip_prefix("win-mountkit = "))
            .and_then(|l| l.split("tag = \"").nth(1))
            .and_then(|l| l.split('"').next())
            .expect("win-mountkit is pinned to a tag");
        let source = format!("git+https://github.com/poeggi/ufsmount?tag={tag}#");
        assert!(lock.contains(&source), "Cargo.lock is not on {tag}");
    }
}

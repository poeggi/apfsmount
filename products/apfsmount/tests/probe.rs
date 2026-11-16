// SPDX-License-Identifier: AGPL-3.0-or-later
//! Volumes are found in images by their magic.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use win_mountkit::device::{Device, Source};
use win_mountkit::fs::FsKind;
use win_mountkit::partition;

/// An image file in the test temp dir, deleted on drop.
struct Image(PathBuf);

impl Image {
    fn new(tag: &str, data: &[u8]) -> Image {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("{tag}-{}.img", std::process::id()));
        std::fs::write(&path, data).unwrap();
        Image(path)
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// What the whole image holds, with `magic` at `at`.
fn probe_whole(tag: &str, at: usize, magic: &[u8]) -> FsKind {
    apfsmount::init();
    let mut data = vec![0u8; 1 << 20];
    data[at..at + magic.len()].copy_from_slice(magic);
    let img = Image::new(tag, &data);
    let dev = Arc::new(Device::open(&Source::Image(img.0.clone()), None).unwrap());
    let cands = partition::probe(&dev);
    let whole = cands.iter().find(|c| c.id == "whole").unwrap();
    whole.fs.clone()
}

/// The magic alone is no container.
#[test]
fn apfs_magic_alone_is_damaged() {
    let fs = probe_whole("apfs", 32, b"NXSB");
    assert!(
        matches!(&fs, FsKind::Unsupported(w) if w == "APFS, damaged"),
        "{fs:?}"
    );
}

/// The signature alone is no volume.
#[test]
fn hfs_plus_signature_alone_is_damaged() {
    let fs = probe_whole("hfsplus", 1024, b"H+");
    assert!(
        matches!(&fs, FsKind::Unsupported(w) if w == "HFS+, damaged"),
        "{fs:?}"
    );
}

#[test]
fn nothing_else_is() {
    let fs = probe_whole("empty", 0, b"");
    assert!(matches!(fs, FsKind::None), "{fs:?}");
}

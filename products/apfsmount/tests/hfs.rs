// SPDX-License-Identifier: AGPL-3.0-or-later
//! HFS+ (HFSX) images made by the hfsplus crate's image builder, read through
//! the volume layer as the mount and `cp` read them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hfsplus::testutil::HfsPlusImageBuilder;
use win_mountkit::device::{Device, Source};
use win_mountkit::names::Charset;
use win_mountkit::partition;
use win_mountkit::volume::Volume;

/// An image file in the test temp dir, deleted on drop.
struct Image(PathBuf);

impl Drop for Image {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn image(tag: &str, data: &[u8]) -> Image {
    let path =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{tag}-{}.img", std::process::id()));
    std::fs::write(&path, data).unwrap();
    Image(path)
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn open(img: &Image) -> Volume {
    apfsmount::init();
    let dev = Arc::new(Device::open(&Source::Image(img.0.clone()), None).unwrap());
    let cands = partition::probe(&dev);
    let cand = partition::select(&cands, None).unwrap().clone();
    let summary = &cand.found().unwrap().summary;
    assert!(summary.starts_with("HFSX \"TestVolume\""), "{summary}");
    Volume::open(dev, &cand, Charset::Utf8).unwrap()
}

fn read_all(vol: &Volume, path: &str) -> Vec<u8> {
    let node = vol.lookup(&w(path)).unwrap();
    let size = vol.attr(&node).unwrap().size as usize;
    let mut buf = vec![0u8; size];
    let mut off = 0;
    while off < size {
        let n = vol.read(node.id, off as u64, &mut buf[off..]).unwrap();
        assert!(n > 0);
        off += n;
    }
    buf
}

#[test]
fn files_are_read() {
    let big: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let mut b = HfsPlusImageBuilder::new();
    b.add_file("hello.txt", b"Hello, HFS+!\n", 0o644)
        .add_file("big.bin", &big, 0o644)
        .add_file("empty", b"", 0o644);
    let img = image("hfs-files", &b.build());
    let vol = open(&img);

    assert_eq!(vol.info.label, b"TestVolume");
    assert_eq!(vol.info.fs, "HFSX");
    let mut names: Vec<String> = vol
        .dir(vol.root())
        .unwrap()
        .entries
        .iter()
        .map(|e| String::from_utf16(&e.name).unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["big.bin", "empty", "hello.txt"]);
    assert_eq!(read_all(&vol, "hello.txt"), b"Hello, HFS+!\n");
    assert_eq!(read_all(&vol, "big.bin"), big);
    assert!(read_all(&vol, "empty").is_empty());

    // A read in the middle, across a block boundary.
    let node = vol.lookup(&w("big.bin")).unwrap();
    let mut buf = vec![0u8; 100];
    assert_eq!(vol.read(node.id, 4050, &mut buf).unwrap(), 100);
    assert_eq!(buf, big[4050..4150]);
}

/// A symlink reads its target, composed like the names; the root is its own parent.
#[test]
fn symlinks_and_parents() {
    let mut b = HfsPlusImageBuilder::new();
    b.add_file("Caf\u{e9}", b"x", 0o644)
        .add_file("link", "Cafe\u{301}".as_bytes(), 0o120777);
    let img = image("hfs-links", &b.build());
    let vol = open(&img);

    let root = vol.dir(vol.root()).unwrap();
    assert_eq!(root.parent, vol.root());
    let node = vol.lookup(&w("link")).unwrap();
    let attr = vol.attr(&node).unwrap();
    assert!(attr.is_symlink);
    assert!(!attr.link_to_dir);
    assert_eq!(vol.readlink(node.id).unwrap(), "Caf\u{e9}".as_bytes());
}

/// A fork that ends before the file does fails the read, never reads short.
#[test]
fn short_fork_is_an_error() {
    let mut b = HfsPlusImageBuilder::new();
    b.add_file_with_declared_size("cut.bin", &[7u8; 100], 0o644, 1 << 20);
    let img = image("hfs-short", &b.build());
    let vol = open(&img);

    let node = vol.lookup(&w("cut.bin")).unwrap();
    let mut buf = vec![0u8; 64 * 1024];
    assert!(vol.read(node.id, 512 * 1024, &mut buf).is_err());
}

/// Names show composed, as macOS shows them, and `/` (allowed in HFS+) as `:`.
#[test]
fn names_are_composed() {
    let mut b = HfsPlusImageBuilder::new();
    b.add_file("Cafe\u{301}", b"x", 0o644)
        .add_file("a/b", b"y", 0o644);
    let img = image("hfs-names", &b.build());
    let vol = open(&img);
    let table = vol.dir(vol.root()).unwrap();
    let mut raw: Vec<String> = table
        .entries
        .iter()
        .map(|e| String::from_utf8(e.raw.clone()).unwrap())
        .collect();
    raw.sort();
    assert_eq!(raw, ["Caf\u{e9}", "a:b"]);
    assert_eq!(read_all(&vol, "Caf\u{e9}"), b"x");
}

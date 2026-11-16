// SPDX-License-Identifier: AGPL-3.0-or-later
//! Finding APFS and HFS+ on a disk by their magic. Every place is checked,
//! whatever its partition type says.

use std::sync::Arc;

use unicode_normalization::UnicodeNormalization;
use win_mountkit::device::Device;
use win_mountkit::fs::{Backend, Found, Fs, FsKind};
use win_mountkit::partition::{Candidate, read};

use crate::{apfs, hfs};

/// `nx_magic` of the APFS container superblock, in block 0.
const NX_MAGIC: usize = 32;
/// The HFS+ volume header, or the HFS master directory block.
const HFS_HEADER: usize = 1024;
/// `drEmbedSigWord` of an HFS wrapper around an HFS+ volume.
const EMBED_SIG: usize = HFS_HEADER + 0x7c;
/// The bytes `kind` looks at.
const HEAD: usize = 1536;
/// The most one request reads from the disk: a reader crate that loops over
/// a crafted disk fails the request here.
pub(crate) const READ_BUDGET: u64 = 512 << 20;

/// What a place holds, by the first `HEAD` bytes.
fn kind(head: &[u8]) -> Option<&'static str> {
    if &head[NX_MAGIC..NX_MAGIC + 4] == b"NXSB" {
        return Some("APFS");
    }
    match &head[HFS_HEADER..HFS_HEADER + 2] {
        b"H+" => Some("HFS+"),
        b"HX" => Some("HFSX"),
        b"BD" if &head[EMBED_SIG..EMBED_SIG + 2] == b"H+" => Some("HFS+ in an HFS wrapper"),
        b"BD" => Some("HFS"),
        _ => None,
    }
}

/// A symlink target with its names composed, as the listings show them.
pub(crate) fn composed(target: Vec<u8>) -> Vec<u8> {
    match String::from_utf8(target) {
        Ok(s) => s.nfc().collect::<String>().into_bytes(),
        Err(e) => e.into_bytes(),
    }
}

/// A volume name for a summary line, without control characters.
pub(crate) fn printable(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn kind_at(dev: &Device, cand: &Candidate) -> Option<&'static str> {
    if cand.len < HEAD as u64 {
        return None;
    }
    read(dev, cand.start, HEAD as u64).as_deref().and_then(kind)
}

pub struct Mac;

impl Backend for Mac {
    fn probe(&self, dev: &Arc<Device>, cand: &Candidate) -> Vec<FsKind> {
        match kind_at(dev, cand) {
            Some("APFS") => apfs::probe(dev, cand),
            Some("HFS") => vec![FsKind::Unsupported("HFS (before HFS+)".into())],
            Some(_) => hfs::probe(dev, cand),
            None => Vec::new(),
        }
    }

    fn open(
        &self,
        dev: Arc<Device>,
        cand: &Candidate,
        found: &Found,
    ) -> anyhow::Result<Box<dyn Fs>> {
        match kind_at(&dev, cand) {
            Some("APFS") => apfs::open(dev, cand, found),
            Some("HFS") | None => anyhow::bail!("{} holds no APFS or HFS+ any more", cand.id),
            Some(_) => hfs::open(dev, cand, found),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(at: usize, magic: &[u8]) -> Vec<u8> {
        let mut b = vec![0; HEAD];
        b[at..at + magic.len()].copy_from_slice(magic);
        b
    }

    #[test]
    fn kinds() {
        assert_eq!(kind(&[0; HEAD]), None);
        assert_eq!(kind(&head(NX_MAGIC, b"NXSB")), Some("APFS"));
        assert_eq!(kind(&head(HFS_HEADER, b"H+")), Some("HFS+"));
        assert_eq!(kind(&head(HFS_HEADER, b"HX")), Some("HFSX"));
        assert_eq!(kind(&head(HFS_HEADER, b"BD")), Some("HFS"));
        let mut wrapper = head(HFS_HEADER, b"BD");
        wrapper[EMBED_SIG..EMBED_SIG + 2].copy_from_slice(b"H+");
        assert_eq!(kind(&wrapper), Some("HFS+ in an HFS wrapper"));
        // The magic counts only at its own place.
        assert_eq!(kind(&head(0, b"NXSB")), None);
        assert_eq!(kind(&head(0, b"H+")), None);
    }
}

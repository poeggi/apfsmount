// SPDX-License-Identifier: AGPL-3.0-or-later
//! APFS through apfs-core (from apfs-explorer): the volumes of a container
//! and reading one of them.
//!
//! Each volume of a container is a file system of its own. Encrypted volumes
//! are listed and refused. System volumes (Preboot, Recovery, VM, Update, ...)
//! are mounted only on request. Snapshots and Fusion containers are not read.

use std::io;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use apfs_core::block_device::{BlockDevice, BlockError};
use apfs_core::catalog::Catalog;
use apfs_core::container::{Container, ContainerError};
use apfs_core::omap::Omap;
use apfs_core::volume::{VolumeRole, VolumeSuperblock};
use apfs_core::xattr::XATTR_DATA_STREAM;
use unicode_normalization::UnicodeNormalization;
use win_mountkit::device::Device;
use win_mountkit::fs::{Found, Fs, FsKind, Info, Kind, NodeId, Stat};
use win_mountkit::partition::Candidate;
use win_mountkit::volume::filetime;

use crate::mac;

const ROOT_DIR: NodeId = apfs_core::jkey::ROOT_DIR_INO_NUM;
/// `nx_block_size`, `nx_block_count` and `nx_xp_desc_blocks` in block 0.
const NX_BLOCK_SIZE: usize = 36;
const NX_BLOCK_COUNT: usize = 40;
const NX_XP_DESC_BLOCKS: usize = 104;
/// The largest checkpoint descriptor area accepted, in blocks. apfs-core
/// reads all of it when it opens the container.
const MAX_XP_DESC_BLOCKS: u32 = 1 << 16;
/// Where a symlink keeps its target.
const SYMLINK_XATTR: &str = "com.apple.fs.symlink";
/// `apfs_fs_quota_block_count` and `apfs_fs_alloc_count` in the volume superblock.
const VSB_QUOTA_BLOCKS: usize = 0x50;
const VSB_ALLOC_BLOCKS: usize = 0x58;

const S_IFMT: u16 = 0o170000;
const S_IFDIR: u16 = 0o040000;
const S_IFREG: u16 = 0o100000;
const S_IFLNK: u16 = 0o120000;
const DT_DIR: u16 = 4;
const DT_REG: u16 = 8;
const DT_LNK: u16 = 10;

/// One place on the disk, as apfs-core reads it, with a budget per request.
struct Part {
    dev: Arc<Device>,
    start: u64,
    len: u64,
    left: u64,
}

impl Part {
    fn new(dev: Arc<Device>, cand: &Candidate) -> Part {
        Part {
            dev,
            start: cand.start,
            len: cand.len,
            left: mac::READ_BUDGET,
        }
    }

    fn fresh(&mut self) {
        self.left = mac::READ_BUDGET;
    }
}

impl BlockDevice for Part {
    fn size(&self) -> u64 {
        self.len
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let len = buf.len() as u64;
        if len > self.left {
            return Err(BlockError::Io(io::Error::other(
                "APFS: a request reads too much",
            )));
        }
        self.left -= len;
        let out = BlockError::OutOfRange {
            offset,
            len,
            size: self.len,
        };
        if offset.checked_add(len).is_none_or(|end| end > self.len) {
            return Err(out);
        }
        self.dev
            .read(self.start + offset, buf)
            .map_err(BlockError::Io)
    }
}

/// A device error stays as it is, so the caller can tell a bad sector.
fn io_error(e: ContainerError) -> io::Error {
    match e {
        ContainerError::Io(BlockError::Io(e)) => e,
        e => io::Error::other(format!("APFS: {e:?}")),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The role for a summary, as " (Preboot)"; nothing for a volume without one.
fn role_name(role: VolumeRole) -> String {
    match role {
        VolumeRole::None => String::new(),
        VolumeRole::Vm => " (VM)".into(),
        VolumeRole::Other(raw) => format!(" (role {raw:#x})"),
        r => format!(" ({r:?})"),
    }
}

/// Whether block 0 claims a container that fits the place and a checkpoint
/// area of a sane size. apfs-core trusts both while it opens the container.
fn plausible(part: &mut Part) -> bool {
    let mut b = [0u8; NX_XP_DESC_BLOCKS + 4];
    if part.read_at(0, &mut b).is_err() {
        return false;
    }
    let at4 = |i: usize| [b[i], b[i + 1], b[i + 2], b[i + 3]];
    let block_size = u32::from_le_bytes(at4(NX_BLOCK_SIZE));
    let mut count = [0u8; 8];
    count.copy_from_slice(&b[NX_BLOCK_COUNT..NX_BLOCK_COUNT + 8]);
    let desc = u32::from_le_bytes(at4(NX_XP_DESC_BLOCKS)) & 0x7fff_ffff;
    u64::from_le_bytes(count)
        .checked_mul(u64::from(block_size))
        .is_some_and(|size| size <= part.len)
        && desc <= MAX_XP_DESC_BLOCKS
}

/// The volumes of the container at `cand`.
pub fn probe(dev: &Arc<Device>, cand: &Candidate) -> Vec<FsKind> {
    let mut part = Part::new(dev.clone(), cand);
    if !plausible(&mut part) {
        log::warn!(
            "{} {}: APFS container header out of bounds",
            dev.name(),
            cand.id
        );
        return vec![FsKind::Unsupported("APFS, damaged".into())];
    }
    let container = match Container::open(&mut part) {
        Ok(c) => c,
        Err(ContainerError::UnsupportedContainer { .. }) => {
            return vec![FsKind::Unsupported("APFS Fusion container".into())];
        }
        Err(e) => {
            log::warn!(
                "{} {}: APFS container not readable: {e:?}",
                dev.name(),
                cand.id
            );
            return vec![FsKind::Unsupported("APFS, damaged".into())];
        }
    };
    let nx = &container.superblock;
    let block_size = nx.block_size;
    let Some(size) = nx.block_count.checked_mul(u64::from(block_size)) else {
        return vec![FsKind::Unsupported("APFS, damaged".into())];
    };
    let volumes = match container.list_volumes(&mut part) {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "{} {}: APFS volumes not readable: {e:?}",
                dev.name(),
                cand.id
            );
            return vec![FsKind::Unsupported("APFS, damaged".into())];
        }
    };
    if volumes.is_empty() {
        return vec![FsKind::Unsupported("APFS, damaged".into())];
    }
    volumes
        .into_iter()
        .map(|v| {
            let what = format!(
                "APFS volume \"{}\"{}",
                mac::printable(&v.name),
                role_name(v.role)
            );
            if v.encrypted {
                FsKind::Unsupported(format!("{what}, encrypted"))
            } else {
                FsKind::Found(Box::new(Found {
                    id: hex(&v.uuid),
                    size,
                    block_size,
                    volume: v.index,
                    on_request: v.system,
                    summary: what,
                }))
            }
        })
        .collect()
}

/// Open the volume `found` names in the container at `cand`.
pub fn open(dev: Arc<Device>, cand: &Candidate, found: &Found) -> anyhow::Result<Box<dyn Fs>> {
    let mut part = Part::new(dev, cand);
    if !plausible(&mut part) {
        anyhow::bail!("{}: APFS container header out of bounds", cand.id);
    }
    let container = Container::open(&mut part).map_err(io_error)?;
    let nx = &container.superblock;
    let block_size = nx.block_size;
    let (xid, omap_oid) = (nx.obj.xid, nx.omap_oid);
    let oid = nx
        .fs_slots
        .iter()
        .find(|&&(slot, _)| slot == found.volume)
        .map(|&(_, oid)| oid)
        .ok_or_else(|| anyhow::anyhow!("{}: the APFS volume is gone", cand.id))?;
    let omap = Omap::open(&mut part, omap_oid, block_size).map_err(io_error)?;
    let paddr = omap
        .resolve(&mut part, oid, xid, block_size)
        .map_err(io_error)?
        .ok_or_else(|| anyhow::anyhow!("{}: the APFS volume is not mapped", cand.id))?;
    let mut raw = vec![0u8; block_size as usize];
    let at = paddr
        .checked_mul(u64::from(block_size))
        .ok_or_else(|| anyhow::anyhow!("{}: bad APFS volume address", cand.id))?;
    part.read_at(at, &mut raw)
        .map_err(|e| io_error(ContainerError::Io(e)))?;
    let vsb = VolumeSuperblock::parse(&raw).map_err(io_error)?;
    if hex(&vsb.uuid) != found.id {
        anyhow::bail!("{}: the APFS volume changed", cand.id);
    }
    if vsb.is_encrypted() {
        anyhow::bail!("{}: the APFS volume \"{}\" is encrypted", cand.id, vsb.name);
    }
    let catalog = Catalog::open(&mut part, &vsb, block_size).map_err(io_error)?;

    let field = |at: usize| {
        raw.get(at..at + 8)
            .and_then(|b| b.try_into().ok())
            .map_or(0, u64::from_le_bytes)
    };
    let bsz = u64::from(block_size);
    let (quota, alloc) = (field(VSB_QUOTA_BLOCKS), field(VSB_ALLOC_BLOCKS));
    let (total, free) = if quota != 0 {
        (quota, quota.saturating_sub(alloc))
    } else if nx.xp_desc_len <= nx.xp_desc_blocks & 0x7fff_ffff {
        let free = container.free_block_count(&mut part).unwrap_or(0);
        (nx.block_count, free.min(nx.block_count))
    } else {
        (nx.block_count, 0)
    };
    let serial = u32::from_le_bytes([vsb.uuid[0], vsb.uuid[1], vsb.uuid[2], vsb.uuid[3]]);
    let info = Info {
        fs: "APFS",
        label: vsb.name.clone().into_bytes(),
        serial,
        block_size,
        total: total.saturating_mul(bsz),
        free: free.saturating_mul(bsz),
        mount_point: Vec::new(),
    };
    Ok(Box::new(ApfsFs {
        part,
        catalog,
        names_hashed: vsb.names_are_hashed(),
        block_size: bsz,
        info,
    }))
}

struct ApfsFs {
    part: Part,
    catalog: Catalog,
    names_hashed: bool,
    block_size: u64,
    info: Info,
}

/// APFS times are nanoseconds since 1970.
fn time(ns: u64) -> u64 {
    filetime(UNIX_EPOCH + Duration::from_nanos(ns))
}

impl Fs for ApfsFs {
    fn info(&self) -> Info {
        self.info.clone()
    }

    fn root(&self) -> NodeId {
        ROOT_DIR
    }

    fn stat(&mut self, id: NodeId) -> io::Result<Stat> {
        self.part.fresh();
        let st = self.catalog.stat(&mut self.part, id).map_err(io_error)?;
        let kind = match st.mode & S_IFMT {
            S_IFDIR => Kind::Dir,
            S_IFREG => Kind::File,
            S_IFLNK => Kind::Symlink,
            _ if st.is_dir => Kind::Dir,
            _ => Kind::Other,
        };
        let nlink = if kind == Kind::Dir {
            1
        } else {
            st.nchildren_or_nlink.max(1)
        };
        Ok(Stat {
            kind,
            size: st.size,
            alloc: st
                .size
                .checked_next_multiple_of(self.block_size)
                .unwrap_or(st.size),
            nlink,
            creation: time(st.create_time),
            access: time(st.access_time),
            write: time(st.mod_time),
            change: time(st.change_time),
        })
    }

    fn read_dir(
        &mut self,
        dir: NodeId,
        f: &mut dyn FnMut(&[u8], NodeId, Kind) -> bool,
    ) -> io::Result<()> {
        self.part.fresh();
        // APFS keeps no `..` entry: the parent comes from the directory's inode.
        let parent = match dir {
            ROOT_DIR => ROOT_DIR,
            _ => self
                .catalog
                .inode_record(&mut self.part, dir)
                .map_err(io_error)?
                .map_or(ROOT_DIR, |i| i.parent_id),
        };
        if f(b"..", parent, Kind::Dir) {
            return Ok(());
        }
        let entries = self
            .catalog
            .list_dir(&mut self.part, dir, self.names_hashed)
            .map_err(io_error)?;
        for e in entries {
            let kind = match e.flags & 0xf {
                DT_DIR => Kind::Dir,
                DT_REG => Kind::File,
                DT_LNK => Kind::Symlink,
                _ => Kind::Other,
            };
            // macOS keeps names as they were given, often decomposed.
            let name: String = e.name.nfc().collect();
            if f(name.as_bytes(), e.file_id, kind) {
                break;
            }
        }
        Ok(())
    }

    fn read(&mut self, id: NodeId, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.part.fresh();
        self.part.left = self.part.left.saturating_add(buf.len() as u64);
        let offset = usize::try_from(offset).map_err(|_| io::Error::from_raw_os_error(22))?;
        let data = self
            .catalog
            .read_file_decompressed_range(&mut self.part, id, offset, buf.len())
            .map_err(io_error)?;
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
    }

    fn read_link(&mut self, id: NodeId) -> io::Result<Vec<u8>> {
        self.part.fresh();
        // macOS stores a target inline. A stream would be read whole, at
        // whatever size the disk claims.
        let flags = self
            .catalog
            .xattr_flags(&mut self.part, id, SYMLINK_XATTR)
            .map_err(io_error)?
            .ok_or_else(|| io::Error::from_raw_os_error(2))?;
        if flags & XATTR_DATA_STREAM != 0 {
            return Err(io::Error::other("APFS: symlink target in a stream"));
        }
        let mut target = self
            .catalog
            .get_xattr(&mut self.part, id, SYMLINK_XATTR)
            .map_err(io_error)?
            .ok_or_else(|| io::Error::from_raw_os_error(2))?;
        while target.last() == Some(&0) {
            target.pop();
        }
        Ok(mac::composed(target))
    }
}

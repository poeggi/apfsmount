// SPDX-License-Identifier: AGPL-3.0-or-later
//! HFS+ and HFSX through the hfsplus crate (dpp), by catalog node id.
//!
//! File data is read through its extents, the overflow extents included.
//! Hard links (files and folders) show their target. Compressed files
//! (decmpfs) are decompressed whole. An HFS wrapper is opened at its embedded
//! HFS+ volume. A journal is not replayed: a volume that was not unmounted
//! cleanly is read as is.

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use hfsplus::btree::{self, BTreeHeaderRecord};
use hfsplus::catalog::{self, CatalogFile, CatalogRecord};
use hfsplus::volume::{ForkData, VolumeHeader};
use hfsplus::{HfsPlusError, attributes};
use unicode_normalization::UnicodeNormalization;
use win_mountkit::device::{Device, PartitionReader};
use win_mountkit::fs::{Found, Fs, FsKind, Info, Kind, NodeId, Stat};
use win_mountkit::partition::{Candidate, be16, be32, read};
use win_mountkit::volume::filetime;

use crate::mac;

const ROOT_FOLDER: u32 = catalog::CNID_ROOT_FOLDER;
/// Where hard-linked files and folders keep their data.
const PRIVATE_FILES: &str = "\0\0\0\0HFS+ Private Data";
const PRIVATE_DIRS: &str = ".HFS+ Private Directory Data\r";
const FORK_DATA: u8 = 0x00;
const FORK_RESOURCE: u8 = 0xff;
/// `kHFSVolumeUnmountedBit` and `kHFSVolumeJournaledBit` in the volume header.
const UNMOUNTED: u32 = 1 << 8;
const JOURNALED: u32 = 1 << 13;
/// `UF_COMPRESSED` in the owner flags.
const UF_COMPRESSED: u8 = 0x20;
/// Catalog records kept, enough for the entries of large directories.
const MAX_CACHED: usize = 65536;
/// The largest compressed file that is decompressed.
const MAX_DECOMPRESSED: u64 = 1 << 30;
/// The most extents read for one fork.
const MAX_EXTENTS: usize = 1 << 20;
/// The largest allocation block accepted.
const MAX_BLOCK_SIZE: u32 = 1 << 24;
/// The longest symlink target read (macOS allows 1024 bytes).
const LINK_MAX: u64 = 4096;
/// Seconds from 1904 (HFS+ times) to 1970.
const EPOCH_1904: u64 = 2_082_844_800;

const S_IFMT: u16 = 0o170000;
const S_IFREG: u16 = 0o100000;
const S_IFLNK: u16 = 0o120000;

/// A fork's extents: (first logical block, first disk block, blocks).
type Extents = Vec<(u64, u64, u64)>;

fn io_error(e: HfsPlusError) -> io::Error {
    match e {
        HfsPlusError::Io(e) => e,
        e => io::Error::other(format!("HFS+: {e}")),
    }
}

/// The catalog node id of a file or folder record.
fn own_id(r: &CatalogRecord) -> Option<u32> {
    match r {
        CatalogRecord::Folder(d) => Some(d.folder_id),
        CatalogRecord::File(f) => Some(f.file_id),
        _ => None,
    }
}

fn not_found() -> io::Error {
    io::Error::from_raw_os_error(2)
}

fn damaged(what: &str) -> io::Error {
    io::Error::other(format!("HFS+: {what}"))
}

/// The byte offset of the HFS+ volume and, inside an HFS wrapper, its length.
/// `head` is the first 1536 bytes of the place.
fn embedded(head: &[u8]) -> Option<(u64, Option<u64>)> {
    match head.get(1024..1026)? {
        b"H+" | b"HX" => Some((0, None)),
        b"BD" if head.get(1024 + 0x7c..1024 + 0x7e)? == b"H+" => {
            let mdb = &head[1024..];
            let block = u64::from(be32(mdb, 0x14));
            let first = u64::from(be16(mdb, 0x1c)) * 512;
            let start = u64::from(be16(mdb, 0x7e));
            let count = u64::from(be16(mdb, 0x80));
            Some((first + start * block, Some(count * block)))
        }
        _ => None,
    }
}

/// The kind of a file record, from its BSD mode.
fn file_kind(mode: u16) -> Kind {
    match mode & S_IFMT {
        0 | S_IFREG => Kind::File,
        S_IFLNK => Kind::Symlink,
        _ => Kind::Other,
    }
}

/// An open HFS+ volume.
struct Volume {
    r: PartitionReader,
    vh: VolumeHeader,
    catalog: BTreeHeaderRecord,
    extents: BTreeHeaderRecord,
    attributes: Option<BTreeHeaderRecord>,
    /// Catalog records listed or looked up last, with their parent folder,
    /// by catalog node id.
    records: HashMap<u32, (u32, CatalogRecord)>,
}

impl Volume {
    fn open(dev: Arc<Device>, cand: &Candidate) -> io::Result<Volume> {
        let head = read(&dev, cand.start, 1536).ok_or_else(|| damaged("unreadable header"))?;
        let (offset, len) = embedded(&head).ok_or_else(|| damaged("no volume header"))?;
        let room = cand
            .len
            .checked_sub(offset)
            .ok_or_else(|| damaged("wrapper past the partition"))?;
        let len = len.unwrap_or(room).min(room);
        let mut r = PartitionReader::new(dev, cand.start + offset, len, 512);
        let vh = VolumeHeader::parse(&mut r).map_err(io_error)?;
        if vh.block_size < 512 || vh.block_size > MAX_BLOCK_SIZE || !vh.block_size.is_power_of_two()
        {
            return Err(damaged("bad block size"));
        }
        let catalog =
            btree::read_btree_header(&mut r, &vh.catalog_file, vh.block_size).map_err(io_error)?;
        let extents =
            btree::read_btree_header(&mut r, &vh.extents_file, vh.block_size).map_err(io_error)?;
        let attributes = if vh.attributes_file.logical_size == 0 {
            None
        } else {
            Some(
                btree::read_btree_header(&mut r, &vh.attributes_file, vh.block_size)
                    .map_err(io_error)?,
            )
        };
        Ok(Volume {
            r,
            vh,
            catalog,
            extents,
            attributes,
            records: HashMap::new(),
        })
    }

    fn lookup(&mut self, parent: u32, name: &str) -> io::Result<Option<CatalogRecord>> {
        catalog::lookup_catalog(&mut self.r, &self.vh, &self.catalog, parent, name)
            .map_err(io_error)
    }

    /// The parent and name of `cnid`, from its thread record.
    fn thread(&mut self, cnid: u32) -> io::Result<(u32, String)> {
        match self.lookup(cnid, "")? {
            Some(CatalogRecord::FileThread(t) | CatalogRecord::FolderThread(t)) => {
                Ok((t.parent_id, t.node_name))
            }
            Some(_) => Err(damaged("a thread record is something else")),
            None => Err(not_found()),
        }
    }

    /// The file and folder records of a folder. Found by parent id alone, so
    /// unlike a lookup by name it does not depend on the name order.
    fn list(&mut self, parent: u32) -> io::Result<Vec<catalog::DirRecord>> {
        catalog::list_directory_records(&mut self.r, &self.vh, &self.catalog, parent)
            .map_err(io_error)
    }

    /// The record of `cnid` and its parent folder.
    fn record_with_parent(&mut self, cnid: u32) -> io::Result<(u32, CatalogRecord)> {
        if let Some(r) = self.records.get(&cnid) {
            return Ok(r.clone());
        }
        let (parent, name) = self.thread(cnid)?;
        // hfsplus orders names unlike macOS for NUL and ignorable characters on
        // case-insensitive volumes, so a lookup by name can miss: then the
        // parent's listing is searched.
        let r = match self.lookup(parent, &name)? {
            Some(r) if own_id(&r) == Some(cnid) => r,
            _ => self
                .list(parent)?
                .into_iter()
                .find(|d| own_id(&d.record) == Some(cnid))
                .map(|d| d.record)
                .ok_or_else(not_found)?,
        };
        self.remember(cnid, parent, r.clone());
        Ok((parent, r))
    }

    fn record(&mut self, cnid: u32) -> io::Result<CatalogRecord> {
        self.record_with_parent(cnid).map(|(_, r)| r)
    }

    /// Keep a record read in a listing or a lookup, which spares the next lookups.
    fn remember(&mut self, cnid: u32, parent: u32, r: CatalogRecord) {
        if self.records.len() >= MAX_CACHED {
            self.records.clear();
        }
        self.records.insert(cnid, (parent, r));
    }

    /// The extents of a fork as (first logical block, first disk block, blocks),
    /// the overflow extents included.
    fn fork_extents(
        &mut self,
        file_id: u32,
        fork: &ForkData,
        fork_type: u8,
    ) -> io::Result<Extents> {
        fn push(out: &mut Extents, next: &mut u64, start: u32, count: u32) {
            out.push((*next, u64::from(start), u64::from(count)));
            *next += u64::from(count);
        }
        let mut out = Vec::new();
        let mut next = 0u64;
        for e in fork.extents.iter().take_while(|e| e.block_count != 0) {
            push(&mut out, &mut next, e.start_block, e.block_count);
        }
        while next < u64::from(fork.total_blocks) {
            let start = u32::try_from(next).map_err(|_| damaged("fork too long"))?;
            let record = self.overflow(file_id, fork_type, start)?;
            let before = next;
            for (s, c) in record.into_iter().take_while(|&(_, c)| c != 0) {
                push(&mut out, &mut next, s, c);
            }
            if next == before {
                return Err(damaged("missing overflow extents"));
            }
            if out.len() > MAX_EXTENTS {
                return Err(damaged("file too fragmented"));
            }
        }
        Ok(out)
    }

    /// The overflow extent record of a fork that starts at block `start`.
    fn overflow(&mut self, file_id: u32, fork_type: u8, start: u32) -> io::Result<Vec<(u32, u32)>> {
        let key = |rec: &[u8]| -> hfsplus::Result<std::cmp::Ordering> {
            let k = rec
                .get(..12)
                .ok_or_else(|| HfsPlusError::InvalidBTree("extent key too short".into()))?;
            let rec_fork = k[2];
            let rec_file = u32::from_be_bytes([k[4], k[5], k[6], k[7]]);
            let rec_start = u32::from_be_bytes([k[8], k[9], k[10], k[11]]);
            Ok(rec_file
                .cmp(&file_id)
                .then(rec_fork.cmp(&fork_type))
                .then(rec_start.cmp(&start)))
        };
        let Some((node, i)) =
            btree::search_btree(&mut self.r, &self.extents, &key).map_err(io_error)?
        else {
            return Ok(Vec::new());
        };
        let rec = node.record_data(i).map_err(io_error)?;
        let data = 2 + usize::from(u16::from_be_bytes([rec[0], rec[1]]));
        let body = rec
            .get(data..data + 64)
            .ok_or_else(|| damaged("extent record too short"))?;
        Ok(body
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| {
                (
                    u32::from_be_bytes([c[0], c[1], c[2], c[3]]),
                    u32::from_be_bytes([c[4], c[5], c[6], c[7]]),
                )
            })
            .collect())
    }

    /// `buf.len()` bytes of a fork at `offset`, or fewer at its end.
    fn read_fork(
        &mut self,
        extents: &[(u64, u64, u64)],
        size: u64,
        offset: u64,
        buf: &mut [u8],
    ) -> io::Result<usize> {
        let bsz = u64::from(self.vh.block_size);
        let end = size.min(offset.saturating_add(buf.len() as u64));
        let mut pos = offset;
        while pos < end {
            let block = pos / bsz;
            let &(first, disk, count) = extents
                .iter()
                .find(|&&(first, _, count)| block >= first && block < first + count)
                .ok_or_else(|| damaged("data past the extents"))?;
            let run_end = (first + count)
                .checked_mul(bsz)
                .ok_or_else(|| damaged("bad extent"))?
                .min(end);
            let at = (disk + (block - first))
                .checked_mul(bsz)
                .and_then(|b| b.checked_add(pos % bsz))
                .ok_or_else(|| damaged("bad extent"))?;
            let n = (run_end - pos) as usize;
            let from = (pos - offset) as usize;
            self.r.seek(SeekFrom::Start(at))?;
            self.r.read_exact(&mut buf[from..from + n])?;
            pos += n as u64;
        }
        Ok((end.max(offset) - offset) as usize)
    }

    fn read_whole_fork(
        &mut self,
        file_id: u32,
        fork: &ForkData,
        fork_type: u8,
    ) -> io::Result<Vec<u8>> {
        if fork.logical_size > MAX_DECOMPRESSED {
            return Err(damaged("fork too large to read whole"));
        }
        let extents = self.fork_extents(file_id, fork, fork_type)?;
        let mut data = vec![0u8; fork.logical_size as usize];
        let n = self.read_fork(&extents, fork.logical_size, 0, &mut data)?;
        data.truncate(n);
        Ok(data)
    }

    /// The decmpfs attribute of a file that is flagged compressed.
    fn decmpfs(&mut self, f: &CatalogFile) -> io::Result<Option<Vec<u8>>> {
        if f.permissions.owner_flags & UF_COMPRESSED == 0 {
            return Ok(None);
        }
        let Some(attrs) = self.attributes.clone() else {
            return Ok(None);
        };
        match attributes::lookup(&mut self.r, &attrs, f.file_id, cmpfs::XATTR_NAME)
            .map_err(io_error)?
        {
            Some(attributes::AttrValue::Inline(v)) => Ok(Some(v)),
            // macOS keeps the attribute inline; its data goes to the resource fork.
            Some(attributes::AttrValue::Fork(_)) => Err(damaged("decmpfs in a fork")),
            None => Ok(None),
        }
    }
}

/// The volume name and a summary line, for `probe`.
fn describe(vol: &mut Volume) -> (String, String) {
    let name = vol.thread(ROOT_FOLDER).map(|(_, n)| n).unwrap_or_default();
    let kind = if vol.vh.is_hfsx { "HFSX" } else { "HFS+" };
    let mut s = format!("{kind} \"{}\"", mac::printable(&name));
    if vol.vh.attributes & JOURNALED != 0 {
        s.push_str(", journaled");
    }
    if vol.vh.attributes & UNMOUNTED == 0 {
        s.push_str(", not unmounted cleanly");
    }
    (name, s)
}

fn volume_id(vh: &VolumeHeader) -> String {
    let (a, b) = (vh.finder_info[6], vh.finder_info[7]);
    if a != 0 || b != 0 {
        format!("{a:08x}{b:08x}")
    } else {
        format!("{:08x}{:08x}", vh.create_date, vh.total_blocks)
    }
}

pub fn probe(dev: &Arc<Device>, cand: &Candidate) -> Vec<FsKind> {
    let mut vol = match Volume::open(dev.clone(), cand) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("{} {}: HFS+ not readable: {e}", dev.name(), cand.id);
            return vec![FsKind::Unsupported("HFS+, damaged".into())];
        }
    };
    let (_, summary) = describe(&mut vol);
    let bsz = vol.vh.block_size;
    let size = u64::from(vol.vh.total_blocks) * u64::from(bsz);
    if size > cand.len {
        return vec![FsKind::Unsupported("HFS+, damaged".into())];
    }
    vec![FsKind::Found(Box::new(Found {
        id: volume_id(&vol.vh),
        size,
        block_size: bsz,
        volume: 0,
        on_request: false,
        summary,
    }))]
}

pub fn open(dev: Arc<Device>, cand: &Candidate, found: &Found) -> anyhow::Result<Box<dyn Fs>> {
    let mut vol = Volume::open(dev, cand)?;
    if volume_id(&vol.vh) != found.id {
        anyhow::bail!("{}: the HFS+ volume changed", cand.id);
    }
    if vol.vh.attributes & UNMOUNTED == 0 {
        log::warn!(
            "the HFS+ volume was not unmounted cleanly. It is read as is; recently written files may be incomplete."
        );
    }
    let (name, _) = describe(&mut vol);
    // From the root's listing: their names start with NUL, which hfsplus
    // orders unlike macOS, so a lookup by name would miss them.
    let (mut private_files, mut private_dirs) = (None, None);
    for d in vol.list(ROOT_FOLDER)? {
        if let CatalogRecord::Folder(f) = &d.record {
            if d.name == PRIVATE_FILES {
                private_files = Some(f.folder_id);
            } else if d.name == PRIVATE_DIRS {
                private_dirs = Some(f.folder_id);
            }
        }
    }
    let bsz = u64::from(vol.vh.block_size);
    let vh = &vol.vh;
    let info = Info {
        fs: if vh.is_hfsx { "HFSX" } else { "HFS+" },
        label: name.into_bytes(),
        serial: vh.finder_info[7] ^ vh.create_date,
        block_size: vh.block_size,
        total: u64::from(vh.total_blocks) * bsz,
        free: u64::from(vh.free_blocks.min(vh.total_blocks)) * bsz,
        mount_point: Vec::new(),
    };
    Ok(Box::new(HfsFs {
        vol,
        private_files,
        private_dirs,
        link_nodes: HashMap::new(),
        info,
        extents: None,
        decompressed: None,
        sizes: HashMap::new(),
    }))
}

struct HfsFs {
    vol: Volume,
    private_files: Option<u32>,
    private_dirs: Option<u32>,
    /// Hard link targets by link number, looked up in the private folders.
    link_nodes: HashMap<u32, Option<Target>>,
    info: Info,
    /// The extents of the file read last.
    extents: Option<(u32, Extents)>,
    /// The compressed file read last, decompressed.
    decompressed: Option<(u32, Vec<u8>)>,
    /// Uncompressed sizes of compressed files, by catalog node id.
    sizes: HashMap<u32, u64>,
}

/// HFS+ times are seconds since 1904, UTC.
fn time(secs: u32) -> u64 {
    let unix = u64::from(secs).saturating_sub(EPOCH_1904);
    filetime(UNIX_EPOCH + Duration::from_secs(unix))
}

/// What a hard link in a directory points to.
#[derive(Clone, Copy)]
enum Target {
    File(u32),
    Dir(u32),
}

impl HfsFs {
    /// The target of a hard link record, if `f` is one. A link is a file
    /// without data whose `special` field numbers a node in a private folder.
    /// A folder link carries an alias in its resource fork.
    fn link_target(&mut self, f: &CatalogFile) -> io::Result<Option<Target>> {
        let n = f.permissions.special;
        if n == 0
            || f.data_fork.logical_size != 0
            || f.permissions.owner_flags & UF_COMPRESSED != 0
            || (self.private_files.is_none() && self.private_dirs.is_none())
        {
            return Ok(None);
        }
        if let Some(&t) = self.link_nodes.get(&n) {
            return Ok(t);
        }
        let mut target = None;
        if let Some(dir) = self.private_files
            && let Some(CatalogRecord::File(t)) = self.vol.lookup(dir, &format!("iNode{n}"))?
        {
            target = Some(Target::File(t.file_id));
        } else if let Some(dir) = self.private_dirs
            && let Some(CatalogRecord::Folder(t)) = self.vol.lookup(dir, &format!("dir_{n}"))?
        {
            target = Some(Target::Dir(t.folder_id));
        }
        if self.link_nodes.len() >= MAX_CACHED {
            self.link_nodes.clear();
        }
        self.link_nodes.insert(n, target);
        Ok(target)
    }

    fn file(&mut self, id: NodeId) -> io::Result<CatalogFile> {
        let cnid = u32::try_from(id).map_err(|_| not_found())?;
        match self.vol.record(cnid)? {
            CatalogRecord::File(f) => Ok(f),
            _ => Err(io::Error::from_raw_os_error(21)),
        }
    }

    /// The size of a file's data: uncompressed for a compressed file.
    fn size(&mut self, f: &CatalogFile) -> io::Result<u64> {
        if let Some(&size) = self.sizes.get(&f.file_id) {
            return Ok(size);
        }
        let Some(attr) = self.vol.decmpfs(f)? else {
            return Ok(f.data_fork.logical_size);
        };
        let size = cmpfs::Header::parse(&attr)
            .map_err(|e| damaged(&e.to_string()))?
            .uncompressed_size;
        if self.sizes.len() >= MAX_CACHED {
            self.sizes.clear();
        }
        self.sizes.insert(f.file_id, size);
        Ok(size)
    }

    fn decompress(&mut self, f: &CatalogFile, attr: &[u8]) -> io::Result<()> {
        let header = cmpfs::Header::parse(attr).map_err(|e| damaged(&e.to_string()))?;
        if header.uncompressed_size > MAX_DECOMPRESSED {
            return Err(damaged("compressed file too large"));
        }
        let rsrc = if header.storage() == cmpfs::Storage::ResourceFork {
            Some(
                self.vol
                    .read_whole_fork(f.file_id, &f.resource_fork, FORK_RESOURCE)?,
            )
        } else {
            None
        };
        let data = cmpfs::decompress(attr, rsrc.as_deref()).map_err(|e| damaged(&e.to_string()))?;
        self.decompressed = Some((f.file_id, data));
        Ok(())
    }

    fn read_data(&mut self, f: &CatalogFile, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let ready = self
            .decompressed
            .as_ref()
            .is_some_and(|(id, _)| *id == f.file_id);
        if !ready && let Some(attr) = self.vol.decmpfs(f)? {
            self.decompress(f, &attr)?;
        }
        if let Some((id, data)) = &self.decompressed
            && *id == f.file_id
        {
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(data.len());
            let n = (data.len() - start).min(buf.len());
            buf[..n].copy_from_slice(&data[start..start + n]);
            return Ok(n);
        }
        if self.extents.as_ref().is_none_or(|(id, _)| *id != f.file_id) {
            let e = self.vol.fork_extents(f.file_id, &f.data_fork, FORK_DATA)?;
            self.extents = Some((f.file_id, e));
        }
        let extents = self.extents.take().map(|(_, e)| e).unwrap_or_default();
        let n = self
            .vol
            .read_fork(&extents, f.data_fork.logical_size, offset, buf);
        self.extents = Some((f.file_id, extents));
        n
    }
}

impl Fs for HfsFs {
    fn info(&self) -> Info {
        self.info.clone()
    }

    fn root(&self) -> NodeId {
        NodeId::from(ROOT_FOLDER)
    }

    fn stat(&mut self, id: NodeId) -> io::Result<Stat> {
        let cnid = u32::try_from(id).map_err(|_| not_found())?;
        let bsz = u64::from(self.vol.vh.block_size);
        let (parent, record) = self.vol.record_with_parent(cnid)?;
        match record {
            CatalogRecord::Folder(d) => Ok(Stat {
                kind: Kind::Dir,
                size: 0,
                alloc: 0,
                nlink: 1,
                creation: time(d.create_date),
                access: time(d.access_date),
                write: time(d.content_mod_date),
                change: time(d.attribute_mod_date),
            }),
            CatalogRecord::File(f) => {
                let kind = file_kind(f.permissions.file_mode);
                let size = self.size(&f)?;
                // A hard link's target in the private folder counts its links
                // in `special`.
                let nlink = if Some(parent) == self.private_files {
                    f.permissions.special.max(1)
                } else {
                    1
                };
                Ok(Stat {
                    kind,
                    size,
                    alloc: u64::from(f.data_fork.total_blocks) * bsz,
                    nlink,
                    creation: time(f.create_date),
                    access: time(f.access_date),
                    write: time(f.content_mod_date),
                    change: time(f.attribute_mod_date),
                })
            }
            _ => Err(damaged("a node is a thread record")),
        }
    }

    fn read_dir(
        &mut self,
        dir: NodeId,
        f: &mut dyn FnMut(&[u8], NodeId, Kind) -> bool,
    ) -> io::Result<()> {
        let cnid = u32::try_from(dir).map_err(|_| not_found())?;
        // HFS+ keeps no `..` entry: the parent comes from the thread record.
        let parent = match cnid {
            ROOT_FOLDER => ROOT_FOLDER,
            _ => self.vol.record_with_parent(cnid)?.0,
        };
        if f(b"..", NodeId::from(parent), Kind::Dir) {
            return Ok(());
        }
        for catalog::DirRecord { name: raw, record } in self.vol.list(cnid)? {
            if cnid == ROOT_FOLDER && (raw == PRIVATE_FILES || raw == PRIVATE_DIRS) {
                continue;
            }
            let (id, kind) = match &record {
                CatalogRecord::Folder(d) => (d.folder_id, Kind::Dir),
                CatalogRecord::File(f) => {
                    // A bad private folder fails only the links, not the listing.
                    let target = self.link_target(f).unwrap_or_else(|e| {
                        log::warn!("HFS+: hard link {}: {e}", f.file_id);
                        None
                    });
                    match target {
                        Some(Target::File(t)) => (t, Kind::File),
                        Some(Target::Dir(t)) => (t, Kind::Dir),
                        None => (f.file_id, file_kind(f.permissions.file_mode)),
                    }
                }
                _ => continue,
            };
            if own_id(&record) == Some(id) {
                self.vol.remember(id, cnid, record);
            }
            // As macOS shows them: composed, and '/' (allowed in HFS+ names) as ':'.
            let name: String = raw.nfc().map(|c| if c == '/' { ':' } else { c }).collect();
            if f(name.as_bytes(), NodeId::from(id), kind) {
                break;
            }
        }
        Ok(())
    }

    fn read(&mut self, id: NodeId, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let file = self.file(id)?;
        self.read_data(&file, offset, buf)
    }

    fn read_link(&mut self, id: NodeId) -> io::Result<Vec<u8>> {
        let file = self.file(id)?;
        let size = self.size(&file)?;
        if size > LINK_MAX {
            return Err(damaged("symlink target too long"));
        }
        let mut target = vec![0u8; size as usize];
        let n = self.read_data(&file, 0, &mut target)?;
        target.truncate(n);
        Ok(mac::composed(target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        // 1904-01-01 + EPOCH_1904 seconds is 1970-01-01.
        assert_eq!(time(EPOCH_1904 as u32), filetime(UNIX_EPOCH));
        assert_eq!(time(0), filetime(UNIX_EPOCH));
    }

    #[test]
    fn wrapper_offset() {
        let mut head = vec![0u8; 1536];
        head[1024..1026].copy_from_slice(b"BD");
        head[1024 + 0x14..1024 + 0x18].copy_from_slice(&4096u32.to_be_bytes());
        head[1024 + 0x1c..1024 + 0x1e].copy_from_slice(&6u16.to_be_bytes());
        head[1024 + 0x7c..1024 + 0x7e].copy_from_slice(b"H+");
        head[1024 + 0x7e..1024 + 0x80].copy_from_slice(&2u16.to_be_bytes());
        head[1024 + 0x80..1024 + 0x82].copy_from_slice(&10u16.to_be_bytes());
        assert_eq!(embedded(&head), Some((6 * 512 + 2 * 4096, Some(10 * 4096))));
        head[1024 + 0x7c..1024 + 0x7e].copy_from_slice(b"\0\0");
        assert_eq!(embedded(&head), None);
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! Walk a file system without WinFsp and time it, twice: the second pass
//! finds the disk data cached. Reads only.
//! `walk <PhysicalDriveN or image> [part] [depth]`

use std::sync::Arc;
use std::time::{Duration, Instant};

use win_mountkit::device::{Device, Source};
use win_mountkit::fs::Kind;
use win_mountkit::names::Charset;
use win_mountkit::partition;
use win_mountkit::volume::{Node, Volume};

fn walk(vol: &Volume, depth: usize) -> String {
    let start = Instant::now();
    let (mut t_dir, mut t_attr) = (Duration::ZERO, Duration::ZERO);
    let (mut dirs, mut entries, mut errors) = (0u64, 0u64, 0u64);
    let (mut links, mut dir_links) = (0u64, 0u64);
    let mut stack = vec![(vol.root(), 0usize)];
    while let Some((id, level)) = stack.pop() {
        let t = Instant::now();
        let table = match vol.dir(id) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("dir {id}: {e}");
                errors += 1;
                continue;
            }
        };
        t_dir += t.elapsed();
        dirs += 1;
        for e in &table.entries {
            entries += 1;
            let t = Instant::now();
            let node = Node {
                id: e.id,
                kind: e.kind,
                parent: id,
            };
            match vol.attr(&node) {
                Ok(a) if a.is_symlink => {
                    links += 1;
                    dir_links += u64::from(a.link_to_dir);
                }
                Ok(_) => {}
                Err(_) => errors += 1,
            }
            t_attr += t.elapsed();
            if e.kind == Kind::Dir && level + 1 < depth {
                stack.push((e.id, level + 1));
            }
        }
    }
    format!(
        "{dirs} dirs, {entries} entries ({links} symlinks, {dir_links} to folders), {errors} errors in {} ms (listing {} ms, attributes {} ms)",
        start.elapsed().as_millis(),
        t_dir.as_millis(),
        t_attr.as_millis()
    )
}

fn main() {
    apfsmount::init();
    let args: Vec<String> = std::env::args().collect();
    let source = args.get(1).expect("walk <disk or image> [part] [depth]");
    let part = args.get(2).map(String::as_str).filter(|p| *p != "-");
    let depth: usize = args
        .get(3)
        .and_then(|d| d.parse().ok())
        .unwrap_or(usize::MAX);

    let dev = Arc::new(Device::open(&Source::parse(source), None).expect("open"));
    let cands = partition::probe(&dev);
    let cand = partition::select(&cands, part).expect("select").clone();
    let vol = Volume::open(dev, &cand, Charset::Utf8).expect("volume");

    println!("cold: {}", walk(&vol, depth));
    println!("warm: {}", walk(&vol, depth));
}

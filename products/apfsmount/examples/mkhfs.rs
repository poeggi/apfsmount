// SPDX-License-Identifier: AGPL-3.0-or-later
//! Write a small HFS+ image for CI's mount test: `mkhfs <out.img>`.

use hfsplus::testutil::HfsPlusImageBuilder;

fn main() {
    let out = std::env::args().nth(1).expect("mkhfs <out.img>");
    let big: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let mut b = HfsPlusImageBuilder::new();
    b.add_file("hello.txt", b"Hello, HFS+!\n", 0o644)
        .add_file("big.bin", &big, 0o644);
    std::fs::write(out, b.build()).expect("write the image");
}

use std::{fs, path::Path};

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn make_icon() -> Vec<u8> {
    const SIZE: usize = 32;
    const XOR_BYTES: usize = SIZE * SIZE * 4;
    const MASK_BYTES: usize = SIZE * 4;
    const DIB_BYTES: usize = 40 + XOR_BYTES + MASK_BYTES;

    let mut out = Vec::with_capacity(22 + DIB_BYTES);

    // ICO header and one 32x32, 32-bit icon directory entry.
    push_u16(&mut out, 0);
    push_u16(&mut out, 1);
    push_u16(&mut out, 1);
    out.extend_from_slice(&[SIZE as u8, SIZE as u8, 0, 0]);
    push_u16(&mut out, 1);
    push_u16(&mut out, 32);
    push_u32(&mut out, DIB_BYTES as u32);
    push_u32(&mut out, 22);

    // BITMAPINFOHEADER. DIB height includes both the color and AND masks.
    push_u32(&mut out, 40);
    push_u32(&mut out, SIZE as u32);
    push_u32(&mut out, (SIZE * 2) as u32);
    push_u16(&mut out, 1);
    push_u16(&mut out, 32);
    push_u32(&mut out, 0);
    push_u32(&mut out, XOR_BYTES as u32);
    push_u32(&mut out, 0);
    push_u32(&mut out, 0);
    push_u32(&mut out, 0);
    push_u32(&mut out, 0);

    // Opaque purple BGRA pixels, stored bottom-up.
    for _ in 0..(SIZE * SIZE) {
        out.extend_from_slice(&[0x9b, 0x4d, 0x2f, 0xff]);
    }

    // Fully transparent AND mask (all bits clear).
    out.resize(out.len() + MASK_BYTES, 0);
    out
}

fn main() {
    let icon_dir = Path::new("icons");
    fs::create_dir_all(icon_dir).expect("create icon directory");
    fs::write(icon_dir.join("icon.ico"), make_icon()).expect("write valid Windows icon");
    tauri_build::build()
}

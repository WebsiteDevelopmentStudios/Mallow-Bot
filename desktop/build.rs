use std::{fs, path::Path};

fn main() {
    let icon_dir = Path::new("icons");
    fs::create_dir_all(icon_dir).expect("create icon directory");
    let icon_path = icon_dir.join("icon.ico");
    if !icon_path.exists() {
        // Minimal valid 1x1, 32-bit ICO. Tauri's Windows resource compiler
        // requires an ICO even when no installer icon is configured.
        let mut ico: Vec<u8> = vec![
            0,0, 1,0, 1,0,
            1,0, 1,0, 0,0, 1,0, 32,0,
            48,0,0,0, 22,0,0,0,
            40,0,0,0, 1,0,0,0, 2,0,0,0,
            1,0, 32,0, 0,0,0,0, 4,0,0,0,
            0,0,0,0, 0,0,0,0, 0,0,0,0,
            0,0,0,0,
            0x9b,0x4d,0x2f,0xff,
            0,0,0,0
        ];
        fs::write(icon_path, &mut ico).expect("write fallback icon");
    }
    tauri_build::build()
}

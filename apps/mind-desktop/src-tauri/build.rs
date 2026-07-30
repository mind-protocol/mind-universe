fn main() {
    #[cfg(target_os = "windows")]
    {
        use std::{env, fs, path::PathBuf};

        // A neutral one-pixel bootstrap icon keeps Windows resource generation
        // deterministic until the graph supplies the product's visual identity.
        const ICON: &[u8] = &[
            0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 32, 0, 48, 0, 0, 0, 22, 0, 0, 0, 40, 0, 0, 0,
            1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 32, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 220, 232, 255, 255, 0, 0, 0, 0,
        ];
        const PNG: &[u8] = &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0,
            0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 8, 215, 99,
            248, 255, 255, 255, 127, 0, 9, 251, 3, 253, 42, 134, 227, 138, 0, 0, 0, 0, 73,
            69, 78, 68, 174, 66, 96, 130,
        ];
        let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
        let icons = manifest.join("icons");
        fs::create_dir_all(&icons).expect("create bootstrap icon directory");
        fs::write(icons.join("icon.png"), PNG).expect("write bootstrap PNG");
        let icon = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("bootstrap.ico");
        fs::write(&icon, ICON).expect("write bootstrap icon");
        let windows = tauri_build::WindowsAttributes::new().window_icon_path(icon);
        let attributes = tauri_build::Attributes::new().windows_attributes(windows);
        tauri_build::try_build(attributes).expect("build Tauri resources");
    }

    #[cfg(not(target_os = "windows"))]
    tauri_build::build();
}

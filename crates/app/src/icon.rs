//! `assets/codenotch.ico`: the application icon, drawn by the same rasteriser as the tray's
//! plain mark (`codenotch_platform::tray::icon`), at the usual Windows icon sizes. The file is
//! committed; `build.rs` embeds it. The test below checks it is up to date and rewrites it when
//! `CODENOTCH_WRITE_ICON=1`.

#[cfg(test)]
mod tests {
    use codenotch_platform::tray::icon::{rasterize, IconSpec};
    use std::path::PathBuf;

    const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

    fn png(rgba: &[u8], size: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, size, size);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Best);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(rgba).unwrap();
        }
        out
    }

    /// ICO container with one PNG image per size (Vista+ format).
    fn ico() -> Vec<u8> {
        let images: Vec<(u32, Vec<u8>)> = SIZES
            .iter()
            .map(|&s| (s, png(&rasterize(&IconSpec::MARK, s), s)))
            .collect();
        let mut out = Vec::new();
        out.extend_from_slice(&[0, 0, 1, 0]);
        out.extend_from_slice(&(images.len() as u16).to_le_bytes());
        let mut offset = 6 + 16 * images.len() as u32;
        for (size, data) in &images {
            let dim = if *size >= 256 { 0 } else { *size as u8 };
            out.extend_from_slice(&[dim, dim, 0, 0]);
            out.extend_from_slice(&1u16.to_le_bytes()); // planes
            out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            offset += data.len() as u32;
        }
        for (_, data) in &images {
            out.extend_from_slice(data);
        }
        out
    }

    #[test]
    fn committed_icon_matches_the_tray_mark() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("codenotch.ico");
        let fresh = ico();
        assert_eq!(&fresh[..4], &[0, 0, 1, 0]);
        if std::env::var("CODENOTCH_WRITE_ICON").as_deref() == Ok("1") {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &fresh).unwrap();
        }
        let committed =
            std::fs::read(&path).expect("assets/codenotch.ico (run with CODENOTCH_WRITE_ICON=1)");
        assert!(
            committed == fresh,
            "assets/codenotch.ico is stale: run the test with CODENOTCH_WRITE_ICON=1"
        );
    }
}

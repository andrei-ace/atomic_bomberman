//! CPU-only validation and decoding for bounded background display workers.
use super::{prepare::check_cancel, Pack, Page};
use anyhow::{ensure, Context, Result};
use image::RgbaImage;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

pub struct PageDecoder {
    root: PathBuf,
    pages: BTreeMap<String, Page>,
}
impl PageDecoder {
    pub fn new(root: &Path, pack: &Pack) -> Self {
        Self {
            root: root.to_path_buf(),
            pages: pack.pages.clone(),
        }
    }
    pub fn decode(&self, name: &str, keyed: bool, cancel: &AtomicBool) -> Result<RgbaImage> {
        check_cancel(cancel)?;
        let page = self.pages.get(name).context("Missing HD display page")?;
        let path = self.root.join(name);
        ensure!(
            page.bytes <= 80 * 1024 * 1024,
            "HD page exceeds the encoded file limit"
        );
        let mut data = Vec::new();
        std::fs::File::open(&path)?
            .take(page.bytes + 1)
            .read_to_end(&mut data)?;
        ensure!(
            data.len() as u64 == page.bytes
                && format!("{:x}", Sha256::digest(&data)) == page.sha256,
            "HD artwork is damaged"
        );
        check_cancel(cancel)?;
        let mut image =
            image::load_from_memory_with_format(&data, image::ImageFormat::Png)?.into_rgba8();
        ensure!(
            image.dimensions() == (page.width, page.height),
            "HD page dimensions changed"
        );
        if keyed {
            for p in image.pixels_mut() {
                if p.0[..3] == [0, 0, 0] {
                    p[3] = 0;
                }
            }
        }
        check_cancel(cancel)?;
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hd::tests::Fixture;
    use std::sync::atomic::Ordering;

    #[test]
    fn display_decodes_original_colors_and_rejects_late_corruption_or_cancellation() {
        let fixture = Fixture::new();
        let decoder = PageDecoder::new(&fixture.root, &fixture.pack);
        let cancel = AtomicBool::new(false);
        let image = decoder.decode("page.png", false, &cancel).unwrap();
        assert_eq!(image.dimensions(), (8, 8));
        assert!(image.pixels().all(|p| p.0 == [10, 20, 30, 255]));

        // Activation validated this file, but it can change before a worker
        // needs it. Verify the bytes actually decoded, not an earlier read.
        std::fs::write(fixture.root.join("page.png"), b"changed after activation").unwrap();
        assert!(decoder
            .decode("page.png", false, &cancel)
            .unwrap_err()
            .to_string()
            .contains("damaged"));
        cancel.store(true, Ordering::Release);
        assert!(decoder
            .decode("page.png", false, &cancel)
            .unwrap_err()
            .to_string()
            .contains("cancel"));
    }
}

//! Optional, locally generated presentation data. Original manifests remain unchanged.
mod color;
mod decode;
mod prepare;
mod resources;
mod scheduler;
mod tool;
mod transfer;

use crate::{source::check_relative, Manifest};
use anyhow::{ensure, Context, Result};
pub use decode::PageDecoder;
pub use prepare::{prepare, prepare_with_workers, preview, Progress};
pub use resources::Resources;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};
pub use transfer::{export_pack, import_pack};

pub const SCALE: u32 = 4;
pub const VERSION: u32 = 2;
pub const MODEL: &str = "realesrgan-x4plus";
pub const PAGE_SIZE: u32 = 2048;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    /// Rectangle in the unchanged original atlas, in logical pixels.
    pub original: [u32; 4],
    pub page: String,
    /// Rectangle in the HD page, in physical texels.
    pub rect: [u32; 4],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pack {
    pub version: u32,
    pub source: String,
    pub scale: u32,
    pub model: String,
    pub images: BTreeMap<String, Vec<Region>>,
    pub pages: BTreeMap<String, Page>,
}

pub fn source_key(manifest: &Manifest) -> Result<String> {
    // Presentation identity deliberately excludes audio and scheme/rule data.
    // A voice pack must not invalidate hours of unchanged HD artwork.
    let mut digest = Sha256::new();
    digest.update(b"atomic-bomberman-hd\0");
    digest.update(VERSION.to_le_bytes());
    digest.update(SCALE.to_le_bytes());
    digest.update(MODEL);
    let mut images: Vec<_> = manifest
        .generated
        .iter()
        .filter(|g| g.path.ends_with(".png"))
        .collect();
    images.sort_by(|a, b| a.path.cmp(&b.path));
    digest.update(serde_json::to_vec(&serde_json::json!({
        "textures":manifest.textures,"animations":manifest.animations,"fonts":manifest.fonts,
        "palette":manifest.palette,"player_remaps":manifest.player_remaps,
        "animation_order":manifest.animation_order,
        "tile_animations":manifest.arenas.iter().map(|a|&a.tiles).collect::<Vec<_>>(),
        "brick_animations":manifest.arenas.iter().map(|a|&a.brick_animation).collect::<Vec<_>>(),
        "images":images,
    }))?);
    Ok(format!("{:x}", digest.finalize()))
}

pub fn cache_path(root: &Path, manifest: &Manifest) -> Result<PathBuf> {
    Ok(root.join(source_key(manifest)?))
}

pub fn variant_key(animation: &crate::AnimationAsset, variant: usize) -> String {
    animation
        .player_variants
        .get(variant)
        .cloned()
        .unwrap_or_else(|| format!("@{}/palette/{variant}", animation.id))
}

pub fn remapped(animation: &crate::AnimationAsset, manifest: &Manifest) -> bool {
    !animation.player_variants.is_empty()
        || manifest
            .arenas
            .iter()
            .any(|a| a.brick_animation == animation.id)
        || [
            "BOMBANIM.ANI",
            "BOMBS.ANI",
            "DUDS.ANI",
            "TRIGBOMB.ANI",
            "TRIGANIM.ANI",
            "FLAME.ANI",
            "MFLAME.ANI",
            "MISC.ANI",
        ]
        .iter()
        .any(|name| {
            animation
                .id
                .rsplit('/')
                .next()
                .is_some_and(|id| id.eq_ignore_ascii_case(name))
        })
}

pub fn file_hash(path: &Path) -> Result<(u64, String)> {
    let mut file = fs::File::open(path)?;
    let mut sha = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut bytes = 0;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        sha.update(&buffer[..n]);
        bytes += n as u64;
    }
    Ok((bytes, format!("{:x}", sha.finalize())))
}

pub(super) fn lock_cache(root: &Path) -> Result<fs::File> {
    let path = root.join("preparation.lock");
    crate::ensure_safe_directory(&path)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    lock.try_lock()
        .context("Another window is using the HD cache; wait for it to finish, then retry")?;
    Ok(lock)
}

pub(super) fn audit_page(path: &Path) -> Result<()> {
    let mut reader =
        png::Decoder::new(std::io::BufReader::new(fs::File::open(path)?)).read_info()?;
    ensure!(
        reader.output_buffer_size() <= 64 * 1024 * 1024,
        "HD page exceeds the decode limit"
    );
    let mut pixels = vec![0; reader.output_buffer_size()];
    reader.next_frame(&mut pixels)?;
    Ok(())
}

impl Pack {
    /// Validate content hashes off the graphics thread before activation.
    pub fn load(root: &Path, manifest: &Manifest) -> Result<Self> {
        crate::ensure_safe_directory(root)?;
        let metadata = root.join("hd.json");
        crate::ensure_safe_directory(&metadata)?;
        ensure!(
            fs::metadata(&metadata)?.len() <= 16 * 1024 * 1024,
            "HD manifest is too large"
        );
        let pack: Self =
            serde_json::from_slice(&fs::read(&metadata)?).context("read HD manifest")?;
        ensure!(
            pack.version == VERSION,
            "HD artwork uses a different processing format. Prepare HD again or import a current HD folder."
        );
        ensure!(
            pack.scale == SCALE && pack.model == MODEL && pack.source == source_key(manifest)?,
            "HD artwork does not match the installed game data"
        );
        ensure!(
            !pack.pages.is_empty() && !pack.images.is_empty(),
            "HD cache is empty"
        );
        let expected = prepare::expected_images(manifest);
        ensure!(
            pack.images.len() == expected.len(),
            "HD cache has missing or extra artwork"
        );
        let mut used_pages = BTreeSet::new();
        for (key, originals) in expected {
            let regions = pack.images.get(&key).context("HD artwork is missing")?;
            ensure!(
                regions.len() == originals.len(),
                "HD frame count differs for {key}"
            );
            for (region, original) in regions.iter().zip(originals) {
                used_pages.insert(region.page.as_str());
                ensure!(
                    region.original == original,
                    "HD frame geometry differs for {key}"
                );
                let page = pack.pages.get(&region.page).context("HD page is missing")?;
                let [x, y, w, h] = region.rect;
                ensure!(
                    w == original[2] * SCALE
                        && h == original[3] * SCALE
                        && x.checked_add(w).is_some_and(|r| r <= page.width)
                        && y.checked_add(h).is_some_and(|r| r <= page.height),
                    "HD frame is outside its texture"
                );
            }
        }
        ensure!(
            used_pages.len() == pack.pages.len(),
            "HD cache contains unreferenced pages"
        );
        for (name, page) in &pack.pages {
            check_relative(name)?;
            ensure!(
                Path::new(name)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("png")),
                "HD pages must be PNG files"
            );
            let path = root.join(name);
            crate::ensure_safe_directory(&path)?;
            ensure!(
                page.width > 0 && page.height > 0 && page.width <= 4096 && page.height <= 4096,
                "HD page exceeds supported dimensions"
            );
            let metadata = fs::metadata(&path)?;
            ensure!(
                metadata.is_file()
                    && metadata.len() == page.bytes
                    && page.bytes <= 80 * 1024 * 1024,
                "HD page is damaged, missing, or exceeds the file-size limit: {name}"
            );
            ensure!(
                file_hash(&path)? == (page.bytes, page.sha256.clone()),
                "HD page is damaged: {name}"
            );
            let decoder = png::Decoder::new(std::io::BufReader::new(fs::File::open(path)?));
            let reader = decoder.read_info()?;
            let info = reader.info();
            ensure!(
                info.width == page.width
                    && info.height == page.height
                    && info.color_type == png::ColorType::Rgba
                    && info.bit_depth == png::BitDepth::Eight,
                "HD page format differs from its manifest"
            );
        }
        Ok(pack)
    }

    /// Explicit offline pixel audit. Normal activation checks every byte's
    /// hash and image header, then decodes only pages actually needed to draw.
    pub fn audit_pixels(&self, root: &Path) -> Result<()> {
        for name in self.pages.keys() {
            audit_page(&root.join(name))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GeneratedAsset, TextureAsset};
    pub(super) struct Fixture {
        pub(super) root: PathBuf,
        pub(super) manifest: Manifest,
        pub(super) pack: Pack,
    }
    impl Fixture {
        pub(super) fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!("atomic-hd-{}-{nonce}-{id}", std::process::id()));
            fs::create_dir(&root).unwrap();
            let mut manifest:Manifest=serde_json::from_value(serde_json::json!({
                "version":1,"textures":[],"animations":[],"fonts":[],"sounds":[],"schemes":[],"arenas":[],"resources":[],"sources":[],"generated":[],"warnings":[],"palette":vec![[0,0,0];256],"player_remaps":vec![(0..=255u8).collect::<Vec<_>>();10],"sound_mappings":[],"animation_order":[]
            })).unwrap();
            manifest.textures.push(TextureAsset {
                id: "test".into(),
                path: "original.png".into(),
                width: 2,
                height: 2,
            });
            manifest.generated.push(GeneratedAsset {
                path: "original.png".into(),
                bytes: 10,
                sha256: "0".repeat(64),
            });
            let image = image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 20, 30, 255]));
            image.save(root.join("page.png")).unwrap();
            let (bytes, sha256) = file_hash(&root.join("page.png")).unwrap();
            let pack = Pack {
                version: VERSION,
                source: source_key(&manifest).unwrap(),
                scale: SCALE,
                model: MODEL.into(),
                images: BTreeMap::from([(
                    "original.png".into(),
                    vec![Region {
                        original: [0, 0, 2, 2],
                        page: "page.png".into(),
                        rect: [0, 0, 8, 8],
                    }],
                )]),
                pages: BTreeMap::from([(
                    "page.png".into(),
                    Page {
                        width: 8,
                        height: 8,
                        bytes,
                        sha256,
                    },
                )]),
            };
            let fixture = Self {
                root,
                manifest,
                pack,
            };
            fixture.save();
            fixture
        }
        pub(super) fn save(&self) {
            fs::write(
                self.root.join("hd.json"),
                serde_json::to_vec(&self.pack).unwrap(),
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn old_generation_formats_cannot_activate_as_color_corrected_packs() {
        let mut f = Fixture::new();
        f.pack.version = VERSION - 1;
        f.save();
        assert!(Pack::load(&f.root, &f.manifest)
            .unwrap_err()
            .to_string()
            .contains("processing format"));
        assert!(f.root.join("page.png").is_file());
    }
    #[test]
    fn hd_cache_binds_to_source_content_and_rejects_corruption() {
        let mut f = Fixture::new();
        assert!(Pack::load(&f.root, &f.manifest).is_ok());
        f.manifest.generated[0].sha256 = "f".repeat(64);
        assert!(Pack::load(&f.root, &f.manifest)
            .unwrap_err()
            .to_string()
            .contains("match"));
        f.pack.source = source_key(&f.manifest).unwrap();
        f.save();
        fs::write(f.root.join("page.png"), b"truncated").unwrap();
        assert!(Pack::load(&f.root, &f.manifest)
            .unwrap_err()
            .to_string()
            .contains("damaged"));
    }
    #[test]
    fn hd_cache_rejects_missing_frames_and_changed_geometry() {
        let mut f = Fixture::new();
        f.pack.images.get_mut("original.png").unwrap()[0].rect = [1, 0, 8, 8];
        f.save();
        assert!(Pack::load(&f.root, &f.manifest)
            .unwrap_err()
            .to_string()
            .contains("outside"));
        f.pack.images.get_mut("original.png").unwrap().clear();
        f.save();
        assert!(Pack::load(&f.root, &f.manifest)
            .unwrap_err()
            .to_string()
            .contains("count"));
    }
    #[test]
    fn hd_cache_rejects_path_traversal() {
        let mut f = Fixture::new();
        let page = f.pack.pages.remove("page.png").unwrap();
        f.pack.pages.insert("../page.png".into(), page);
        f.pack.images.get_mut("original.png").unwrap()[0].page = "../page.png".into();
        f.save();
        assert!(Pack::load(&f.root, &f.manifest).is_err());
    }

    #[test]
    fn voice_and_scheme_mods_share_hd_but_artwork_fonts_and_palette_changes_do_not() {
        let f = Fixture::new();
        let base = source_key(&f.manifest).unwrap();
        let mut modified = f.manifest.clone();
        modified.generated.push(GeneratedAsset {
            path: "sounds/voice.wav".into(),
            bytes: 0,
            sha256: "a".repeat(64),
        });
        modified.generated.push(GeneratedAsset {
            path: "tables/map.SCH.txt".into(),
            bytes: 20,
            sha256: "b".repeat(64),
        });
        modified.sources.clear();
        modified.warnings.push("voice mod".into());
        assert_eq!(source_key(&modified).unwrap(), base);
        assert!(Pack::load(&f.root, &modified).is_ok());
        modified.player_remaps[4][20] = 21;
        assert_ne!(source_key(&modified).unwrap(), base);
        assert!(Pack::load(&f.root, &modified).is_err());
        let mut modified = f.manifest.clone();
        modified.textures[0].width = 3;
        assert_ne!(source_key(&modified).unwrap(), base);
        let mut modified = f.manifest.clone();
        modified.generated[0].sha256 = "c".repeat(64);
        assert_ne!(source_key(&modified).unwrap(), base);
        let mut modified = f.manifest.clone();
        modified.generated.push(GeneratedAsset {
            path: "fonts/custom.png".into(),
            bytes: 30,
            sha256: "d".repeat(64),
        });
        assert_ne!(source_key(&modified).unwrap(), base);
        let mut modified = f.manifest.clone();
        modified.arenas.push(crate::Arena {
            id: 0,
            name: "Arena".into(),
            background: "test".into(),
            tiles: "tile animation".into(),
            brick_animation: "brick animation".into(),
            extras: vec![],
            ice_delay_ms: 0,
            regeneration_seconds: 0,
        });
        let arena_key = source_key(&modified).unwrap();
        modified.arenas[0].ice_delay_ms = 10;
        modified.arenas[0].name = "Translated arena".into();
        assert_eq!(source_key(&modified).unwrap(), arena_key);
        modified.arenas[0].tiles = "different tile context".into();
        assert_ne!(source_key(&modified).unwrap(), arena_key);
    }
}

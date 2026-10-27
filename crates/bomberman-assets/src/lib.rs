//! Original-disc asset import and validated native cache.
mod formats;
mod manifest;
mod source;
pub use manifest::*;

use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Import a CD image or extracted game directory. The source is opened read-only.
/// Output must be new or empty, preventing accidental replacement of user files.
/// A manifest is published only after every source asset has decoded successfully.
pub fn import(source_path: &Path, output: &Path) -> Result<Manifest> {
    let source_path = fs::canonicalize(source_path).context("source does not exist")?;
    ensure_safe_directory(output)?;
    if output.exists() {
        ensure!(
            output.is_dir() && fs::read_dir(output)?.next().is_none(),
            "output must be a new or empty directory: {}",
            output.display()
        );
    }
    let output_absolute = absolute_normalized(output)?;
    ensure!(
        output_absolute != source_path
            && !(source_path.is_dir() && output_absolute.starts_with(&source_path)),
        "output must be outside the source directory"
    );
    let parent = output_absolute
        .parent()
        .context("output must name a cache directory")?;
    fs::create_dir_all(parent)?;
    let parent = fs::canonicalize(parent)?;
    let output = parent.join(
        output_absolute
            .file_name()
            .context("output must name a cache directory")?,
    );
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let stage_path = parent.join(format!(".bomberman-import-{}-{nonce}", std::process::id()));
    fs::create_dir(&stage_path)?;
    struct Staging(PathBuf);
    impl Drop for Staging {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let stage = Staging(stage_path);
    let mut source = source::Source::open(&source_path)?;
    let manifest = import_inner(&mut source, &stage.0)?;
    fs::write(
        stage.0.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    // Validate metadata, generated file hashes, and decoders before publishing.
    validate(&stage.0)?;
    if output.exists() {
        fs::remove_dir(&output)?;
    }
    fs::rename(&stage.0, &output).context("publish imported asset cache")?;
    Ok(manifest)
}

fn import_inner(source: &mut source::Source, output: &Path) -> Result<Manifest> {
    let palette_data = source.read("COLOR.PAL")?;
    let palette = formats::Palette::parse(&palette_data)?;
    let master = formats::resource("DATA/ANI/MASTER.ALI", &source.read("DATA/ANI/MASTER.ALI")?)?;
    let active_animations: BTreeSet<String> = master
        .commands
        .iter()
        .filter(|c| c.kind.starts_with('-'))
        .map(|c| {
            format!(
                "DATA/ANI/{}",
                c.kind.trim_start_matches('-').to_ascii_uppercase()
            )
        })
        .collect();
    let mut manifest = Manifest {
        version: CACHE_VERSION,
        textures: vec![],
        animations: vec![],
        fonts: vec![],
        sounds: vec![],
        schemes: vec![],
        arenas: vec![],
        resources: vec![],
        sources: vec![source_record("COLOR.PAL", &palette_data)],
        generated: vec![],
        warnings: vec![],
        palette: palette.rgb.clone(),
        player_remaps: vec![],
        sound_mappings: vec![],
        animation_order: vec![],
    };
    for index in 0..10 {
        let id = format!("{index}.RMP");
        let data = source
            .read(&id)
            .with_context(|| format!("player color remap {id} missing"))?;
        ensure!(
            data.len() == 259,
            "{id}: expected 256 remap bytes and three color percentages"
        );
        let remap = data[..256]
            .iter()
            .enumerate()
            .map(|(i, &v)| if v == 0 { i as u8 } else { v })
            .collect();
        manifest.player_remaps.push(remap);
        manifest.sources.push(source_record(&id, &data));
    }
    let paths = source.paths();
    for path in &paths {
        let ext = path.rsplit('.').next().unwrap_or_default();
        let is_table = path.starts_with("DATA/RES/") && ext == "RES"
            || path == "MESSAGES.TXT"
            || path == "DATA/ANI/MASTER.ALI"
            || ext == "BM";
        if !matches!(ext, "ANI" | "PCX" | "FON" | "RSS" | "SCH") && !is_table {
            continue;
        }
        let data = source
            .read(path)
            .with_context(|| format!("read source asset {path}"))?;
        manifest.sources.push(source_record(path, &data));
        match ext {
            "ANI" => {
                let decoded = match formats::animation(&data, &palette) {
                    Ok(decoded) => decoded,
                    Err(error) if !active_animations.contains(path) => {
                        manifest.warnings.push(format!("{path}: unused by MASTER.ALI; preserved original because its animation metadata is invalid: {error:#}"));
                        write_generated(
                            output,
                            &format!("unused/{path}"),
                            &data,
                            &mut manifest.generated,
                        )?;
                        continue;
                    }
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("decode required battle animation {path}"))
                    }
                };
                let colored = [
                    "WALK", "STAND", "BOMBWALK", "BPICKUP", "KICK", "PUNCH", "PUP1", "PUP2",
                    "PUP3", "PUP4", "BWALK1", "BWALK2", "BWALK3", "BWALK4", "PUNBOMB1", "PUNBOMB2",
                    "PUNBOMB3", "PUNBOMB4",
                ]
                .iter()
                .any(|stem| path.rsplit('/').next() == Some(format!("{stem}.ANI").as_str()))
                    || path
                        .rsplit('/')
                        .next()
                        .is_some_and(|n| n.starts_with("XPLODE"));
                let a = write_animation(
                    output,
                    path,
                    decoded,
                    &palette,
                    if colored {
                        &manifest.player_remaps
                    } else {
                        &[]
                    },
                    &mut manifest.generated,
                )?;
                manifest.animations.push(a);
            }
            "PCX" => {
                let decoded = formats::pcx(&data).with_context(|| format!("decode PCX {path}"))?;
                let target = format!("images/{path}.png");
                write_png(
                    output,
                    &target,
                    decoded.width,
                    decoded.height,
                    png::ColorType::Rgba,
                    &decoded.rgba,
                    &mut manifest.generated,
                )?;
                manifest.textures.push(TextureAsset {
                    id: path.clone(),
                    path: target,
                    width: decoded.width,
                    height: decoded.height,
                });
            }
            "FON" => {
                let font = formats::font(&data).with_context(|| format!("decode font {path}"))?;
                manifest
                    .fonts
                    .push(write_font(output, path, font, &mut manifest.generated)?);
            }
            "RSS" => {
                let wav =
                    formats::wav(&data).with_context(|| format!("decode raw audio {path}"))?;
                if data.len() % 4 != 0 {
                    manifest.warnings.push(format!("{path}: original contains a final left-channel sample; padded its missing right-channel sample with zero"));
                }
                let target = format!("sounds/{path}.wav");
                write_generated(output, &target, &wav, &mut manifest.generated)?;
                manifest.sounds.push(SoundAsset {
                    id: path.clone(),
                    path: target,
                    sample_rate: 22050,
                    channels: 2,
                    samples_per_channel: data.len().div_ceil(4) as u64,
                });
            }
            "SCH" => manifest.schemes.push(
                formats::scheme(path, &data).with_context(|| format!("decode scheme {path}"))?,
            ),
            _ if is_table => {
                let table = formats::resource(path, &data)?;
                let target = format!("tables/{path}.txt");
                write_generated(output, &target, &data, &mut manifest.generated)?;
                manifest.resources.push(table);
            }
            _ => unreachable!(),
        }
    }
    ensure!(!manifest.schemes.is_empty(), "source has no battle schemes");
    let table = |name: &str| manifest.resources.iter().find(|t| t.id == name);
    if let Some(list) = table("DATA/ANI/MASTER.ALI") {
        for command in &list.commands {
            let name = command.kind.trim_start_matches('-');
            if name.is_empty() {
                continue;
            }
            let id = format!("DATA/ANI/{}", name.to_ascii_uppercase());
            ensure!(
                manifest.animation(&id).is_some(),
                "MASTER.ALI animation missing from source: {id}"
            );
            manifest.animation_order.push(id);
        }
    } else {
        bail!("source has no DATA/ANI/MASTER.ALI animation list");
    }
    if let Some(list) = table("DATA/RES/SOUNDLST.RES") {
        for command in &list.commands {
            if let (Ok(number), Some(name)) = (command.kind.parse(), command.fields.first()) {
                let sound = format!("DATA/SOUND/{}.RSS", name.to_ascii_uppercase());
                if manifest.sound(&sound).is_none()
                    && [(1099, "XXX"), (2900, "0"), (3070, "ZAHPU111")]
                        .iter()
                        .any(|(n, stem)| *n == number && name.eq_ignore_ascii_case(stem))
                {
                    manifest.warnings.push(format!("SOUNDLST.RES entry {number},{name}: placeholder or missing on the supplied disc; retained in the source resource table, omitted from playable sound mappings"));
                    continue;
                }
                ensure!(
                    manifest.sound(&sound).is_some(),
                    "SOUNDLST.RES refers to missing audio: {sound}"
                );
                manifest.sound_mappings.push(SoundMapping { number, sound });
            }
        }
    } else {
        bail!("source has no DATA/RES/SOUNDLST.RES");
    }
    let values: BTreeMap<u32, u32> = table("DATA/RES/VALUELST.RES")
        .context("source has no VALUELST.RES")?
        .commands
        .iter()
        .filter_map(|c| Some((c.kind.parse().ok()?, c.fields.first()?.parse().ok()?)))
        .collect();
    let names: BTreeMap<u32, String> = table("MESSAGES.TXT")
        .context("source has no MESSAGES.TXT")?
        .commands
        .iter()
        .filter_map(|c| Some((c.kind.parse().ok()?, c.fields.first()?.clone())))
        .collect();
    for id in 0..11u8 {
        let background = format!("DATA/RES/FIELD{id}.PCX");
        let tiles = format!("DATA/ANI/TILES{id}.ANI");
        let brick_animation = format!("DATA/ANI/XBRICK{id}.ANI");
        ensure!(
            manifest.texture(&background).is_some()
                && manifest.animation(&tiles).is_some()
                && manifest.animation(&brick_animation).is_some(),
            "arena {id} is missing artwork"
        );
        let extra = format!("DATA/RES/EXTRA{id}.RES");
        manifest.arenas.push(Arena {
            id,
            name: names
                .get(&(150 + u32::from(id)))
                .cloned()
                .unwrap_or_else(|| format!("Arena {}", id + 1)),
            background,
            tiles,
            brick_animation,
            extras: table(&extra).map_or_else(Vec::new, |t| t.commands.clone()),
            ice_delay_ms: values.get(&(450 + u32::from(id))).copied().unwrap_or(0),
            regeneration_seconds: values.get(&(340 + u32::from(id))).copied().unwrap_or(0),
        });
    }
    manifest.warnings.push("CLASSICS.ANI is an unused indexed-color alternative excluded by MASTER.ALI; its private colors are quantized through the game's global RGB555 lookup".to_owned());
    Ok(manifest)
}

fn source_record(path: &str, data: &[u8]) -> SourceAsset {
    SourceAsset {
        path: path.to_owned(),
        bytes: data.len() as u64,
        sha256: sha(data),
    }
}
fn sha(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
fn write_generated(
    root: &Path,
    relative: &str,
    data: &[u8],
    generated: &mut Vec<GeneratedAsset>,
) -> Result<()> {
    source::check_relative(relative)?;
    let target = root.join(relative);
    fs::create_dir_all(target.parent().context("generated file has no parent")?)?;
    fs::write(target, data)?;
    generated.push(GeneratedAsset {
        path: relative.to_owned(),
        bytes: data.len() as u64,
        sha256: sha(data),
    });
    Ok(())
}
fn write_png(
    root: &Path,
    relative: &str,
    width: u32,
    height: u32,
    color: png::ColorType,
    data: &[u8],
    generated: &mut Vec<GeneratedAsset>,
) -> Result<()> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.write_header()?.write_image_data(data)?;
    }
    write_generated(root, relative, &bytes, generated)
}

fn atlas_layout(dims: &[(u32, u32)]) -> Result<(u32, u32, Vec<[u32; 2]>)> {
    ensure!(!dims.is_empty(), "cannot pack empty atlas");
    let max_width = dims.iter().map(|d| d.0).max().unwrap().max(1);
    let total: u64 = dims
        .iter()
        .map(|&(w, h)| u64::from(w + 2) * u64::from(h + 2))
        .sum();
    let ideal = ((total as f64).sqrt().ceil() as u32).next_power_of_two();
    let width = ideal
        .clamp(64, 2048)
        .max((max_width + 2).next_power_of_two());
    ensure!(width <= 8192, "atlas sprite width exceeds limit");
    let mut positions = Vec::with_capacity(dims.len());
    let mut x = 1;
    let mut y = 1;
    let mut row = 0;
    for &(w, h) in dims {
        if x + w + 1 > width {
            x = 1;
            y += row + 2;
            row = 0;
        }
        positions.push([x, y]);
        x += w + 2;
        row = row.max(h);
    }
    let height = (y + row + 1).next_power_of_two();
    ensure!(
        height <= 8192 && u64::from(width) * u64::from(height) <= 32 * 1024 * 1024,
        "animation atlas exceeds size limit"
    );
    Ok((width, height, positions))
}
fn write_animation(
    root: &Path,
    id: &str,
    decoded: formats::Animation,
    palette: &formats::Palette,
    remaps: &[Vec<u8>],
    generated: &mut Vec<GeneratedAsset>,
) -> Result<AnimationAsset> {
    let dimensions: Vec<_> = decoded.frames.iter().map(|f| (f.width, f.height)).collect();
    let (width, height, positions) = atlas_layout(&dimensions)?;
    let mut indexed = vec![0; width as usize * height as usize];
    let mut frames = Vec::with_capacity(decoded.frames.len());
    for (i, f) in decoded.frames.iter().enumerate() {
        let [x, y] = positions[i];
        for row in 0..f.height as usize {
            let start = (y as usize + row) * width as usize + x as usize;
            indexed[start..start + f.width as usize]
                .copy_from_slice(&f.indices[row * f.width as usize..(row + 1) * f.width as usize]);
        }
        frames.push(AnimationFrame {
            index: i as u32,
            x,
            y,
            width: f.width,
            height: f.height,
            origin_x: f.origin_x,
            origin_y: f.origin_y,
            duration_ms: 30,
            source_name: f.source_name.clone(),
        });
    }
    let texture = format!("animations/{id}.png");
    let indexed_path = format!("animations/{id}.indices.png");
    write_png(
        root,
        &texture,
        width,
        height,
        png::ColorType::Rgba,
        &palette.rgba(&indexed, None, true),
        generated,
    )?;
    write_png(
        root,
        &indexed_path,
        width,
        height,
        png::ColorType::Grayscale,
        &indexed,
        generated,
    )?;
    let mut player_variants = Vec::new();
    for (i, remap) in remaps.iter().enumerate() {
        let relative = format!("animations/{id}.player{i}.png");
        write_png(
            root,
            &relative,
            width,
            height,
            png::ColorType::Rgba,
            &palette.rgba(&indexed, Some(remap), true),
            generated,
        )?;
        player_variants.push(relative);
    }
    Ok(AnimationAsset {
        id: id.to_owned(),
        texture,
        indexed_path,
        player_variants,
        width,
        height,
        frames,
        sequences: decoded.sequences,
    })
}
fn write_font(
    root: &Path,
    id: &str,
    font: formats::BitmapFont,
    generated: &mut Vec<GeneratedAsset>,
) -> Result<FontAsset> {
    let dims: Vec<_> = font
        .glyphs
        .iter()
        .map(|(_, w, _)| (*w, font.line_height))
        .collect();
    let (width, height, positions) = atlas_layout(&dims)?;
    let mut pixels = vec![0; width as usize * height as usize * 4];
    let mut glyphs = Vec::new();
    for (i, (codepoint, w, rgba)) in font.glyphs.iter().enumerate() {
        let [x, y] = positions[i];
        for row in 0..font.line_height as usize {
            let start = ((y as usize + row) * width as usize + x as usize) * 4;
            let source = row * *w as usize * 4;
            pixels[start..start + *w as usize * 4]
                .copy_from_slice(&rgba[source..source + *w as usize * 4]);
        }
        glyphs.push(Glyph {
            codepoint: *codepoint,
            x,
            y,
            width: *w,
            height: font.line_height,
            advance: *w + font.spacing,
        });
    }
    let texture = format!("fonts/{id}.png");
    write_png(
        root,
        &texture,
        width,
        height,
        png::ColorType::Rgba,
        &pixels,
        generated,
    )?;
    Ok(FontAsset {
        id: id.to_owned(),
        texture,
        width,
        height,
        line_height: font.line_height,
        spacing: font.spacing,
        glyphs,
    })
}

fn ensure_safe_directory(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if let Ok(meta) = fs::symlink_metadata(&current) {
            ensure!(
                !meta.file_type().is_symlink(),
                "cache path contains a symlink: {}",
                current.display()
            );
        }
    }
    Ok(())
}

fn absolute_normalized(path: &Path) -> Result<PathBuf> {
    use std::path::Component;
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => (),
            Component::ParentDir => {
                ensure!(normalized.pop(), "output escapes filesystem root");
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

/// Load manifest metadata for startup. Use `validate` for a complete cache audit.
pub fn load_manifest(cache: &Path) -> Result<Manifest> {
    ensure_safe_directory(cache)?;
    let p = cache.join("manifest.json");
    ensure_safe_directory(&p)?;
    ensure!(
        fs::metadata(&p)?.len() <= 64 * 1024 * 1024,
        "manifest exceeds size limit"
    );
    let manifest: Manifest = serde_json::from_reader(BufReader::new(File::open(&p)?))
        .context("invalid asset manifest")?;
    check_manifest(&manifest)?;
    Ok(manifest)
}

/// Audit every generated file's size and SHA-256 and decode every PNG/WAV header.
pub fn validate(cache: &Path) -> Result<Manifest> {
    let manifest = load_manifest(cache)?;
    let mut expected_images = BTreeMap::new();
    for t in &manifest.textures {
        expected_images.insert(t.path.as_str(), (t.width, t.height, png::ColorType::Rgba));
    }
    for a in &manifest.animations {
        expected_images.insert(
            a.texture.as_str(),
            (a.width, a.height, png::ColorType::Rgba),
        );
        expected_images.insert(
            a.indexed_path.as_str(),
            (a.width, a.height, png::ColorType::Grayscale),
        );
        for p in &a.player_variants {
            expected_images.insert(p.as_str(), (a.width, a.height, png::ColorType::Rgba));
        }
    }
    for f in &manifest.fonts {
        expected_images.insert(
            f.texture.as_str(),
            (f.width, f.height, png::ColorType::Rgba),
        );
    }
    for asset in &manifest.generated {
        let path = cache.join(&asset.path);
        ensure_safe_directory(&path)?;
        ensure!(
            fs::metadata(&path)?.is_file() && fs::metadata(&path)?.len() == asset.bytes,
            "cache file missing or wrong length: {}",
            asset.path
        );
        let mut reader = BufReader::new(File::open(&path)?);
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        ensure!(
            format!("{:x}", hash.finalize()) == asset.sha256,
            "cache checksum mismatch: {}",
            asset.path
        );
        if asset.path.ends_with(".png") {
            let decoder = png::Decoder::new(BufReader::new(File::open(&path)?));
            let mut reader = decoder
                .read_info()
                .with_context(|| format!("invalid PNG {}", asset.path))?;
            if let Some(&(width, height, color)) = expected_images.get(asset.path.as_str()) {
                let info = reader.info();
                ensure!(
                    info.width == width
                        && info.height == height
                        && info.color_type == color
                        && info.bit_depth == png::BitDepth::Eight,
                    "PNG dimensions/format disagree with manifest: {}",
                    asset.path
                );
            }
            ensure!(
                reader.output_buffer_size() <= 128 * 1024 * 1024,
                "PNG exceeds image size limit"
            );
            let mut decoded = vec![0; reader.output_buffer_size()];
            reader.next_frame(&mut decoded)?;
        } else if asset.path.ends_with(".wav") {
            let mut f = File::open(&path)?;
            let mut header = [0; 44];
            f.read_exact(&mut header)?;
            ensure!(
                &header[..4] == b"RIFF"
                    && &header[8..16] == b"WAVEfmt "
                    && &header[36..40] == b"data"
                    && formats::u32le(&header, 24)? == 22050
                    && formats::u16le(&header, 20)? == 1
                    && formats::u16le(&header, 22)? == 2
                    && formats::u16le(&header, 34)? == 16
                    && u64::from(formats::u32le(&header, 40)?) + 44 == asset.bytes
                    && u64::from(formats::u32le(&header, 4)?) + 8 == asset.bytes
                    && formats::u16le(&header, 32)? == 4
                    && formats::u32le(&header, 28)? == 88200
                    && asset.bytes % 4 == 0,
                "invalid imported WAV {}",
                asset.path
            );
            if let Some(sound) = manifest.sounds.iter().find(|s| s.path == asset.path) {
                ensure!(
                    u64::from(formats::u32le(&header, 40)?) / 4 == sound.samples_per_channel,
                    "WAV sample count differs from manifest"
                );
            }
        }
    }
    Ok(manifest)
}

fn check_manifest(m: &Manifest) -> Result<()> {
    ensure!(
        m.version == CACHE_VERSION,
        "unsupported cache version {}",
        m.version
    );
    ensure!(
        m.palette.len() == 256
            && m.player_remaps.len() == 10
            && m.player_remaps.iter().all(|r| r.len() == 256),
        "invalid palette/remap lengths"
    );
    let mut generated = BTreeSet::new();
    for g in &m.generated {
        source::check_relative(&g.path)?;
        ensure!(
            generated.insert(g.path.as_str()),
            "duplicate generated asset {}",
            g.path
        );
        ensure!(
            g.sha256.len() == 64 && g.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid generated SHA-256"
        );
    }
    for s in &m.sources {
        source::check_relative(&s.path)?;
    }
    let required = |p: &str| -> Result<()> {
        source::check_relative(p)?;
        ensure!(
            generated.contains(p),
            "manifest references unlisted generated file {p}"
        );
        Ok(())
    };
    let bounds = |x: u32, y: u32, w: u32, h: u32, width: u32, height: u32| -> Result<()> {
        ensure!(
            width > 0
                && height > 0
                && width <= 8192
                && height <= 8192
                && u64::from(width) * u64::from(height) <= 32 * 1024 * 1024
                && x.checked_add(w).is_some_and(|r| r <= width)
                && y.checked_add(h).is_some_and(|b| b <= height),
            "sprite outside atlas bounds"
        );
        Ok(())
    };
    for t in &m.textures {
        required(&t.path)?;
        bounds(0, 0, t.width, t.height, t.width, t.height)?;
    }
    for a in &m.animations {
        required(&a.texture)?;
        required(&a.indexed_path)?;
        ensure!(
            a.player_variants.is_empty() || a.player_variants.len() == 10,
            "animation must contain all ten player variants"
        );
        for p in &a.player_variants {
            required(p)?;
        }
        ensure!(!a.frames.is_empty(), "animation has no frames");
        for (i, f) in a.frames.iter().enumerate() {
            ensure!(
                f.index == i as u32 && f.width > 0 && f.height > 0,
                "invalid animation frame index or dimensions"
            );
            bounds(f.x, f.y, f.width, f.height, a.width, a.height)?;
        }
        for s in &a.sequences {
            ensure!(
                !s.frames.is_empty()
                    && s.frames.len() == s.offsets.len()
                    && s.frames.len() == s.durations_ms.len()
                    && s.frames.len() == s.states.len(),
                "invalid animation sequence arrays"
            );
            for (i, st) in s.states.iter().enumerate() {
                ensure!(
                    st.duration_ms > 0
                        && st.duration_ms == s.durations_ms[i]
                        && !st.parts.is_empty()
                        && st.parts[0].frame == s.frames[i],
                    "invalid animation state"
                );
                for p in &st.parts {
                    ensure!(
                        (p.frame as usize) < a.frames.len(),
                        "animation state frame out of range"
                    );
                }
            }
        }
    }
    for f in &m.fonts {
        required(&f.texture)?;
        for g in &f.glyphs {
            bounds(g.x, g.y, g.width, g.height, f.width, f.height)?;
        }
    }
    for s in &m.sounds {
        required(&s.path)?;
        ensure!(
            s.channels == 2 && s.sample_rate == 22050,
            "unsupported sound format"
        );
    }
    for s in &m.schemes {
        formats::scheme(&s.id, s.text.as_bytes())?;
    }
    ensure!(
        m.arenas.len() == 11 && !m.schemes.is_empty(),
        "cache must contain all eleven arenas and battle schemes"
    );
    for a in &m.arenas {
        ensure!(
            m.texture(&a.background).is_some()
                && m.animation(&a.tiles).is_some()
                && m.animation(&a.brick_animation).is_some(),
            "arena references missing artwork"
        );
    }
    for s in &m.sound_mappings {
        ensure!(
            m.sound(&s.sound).is_some(),
            "sound mapping refers to missing sound"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "bomberman-import-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn save(root: &Path, name: &str, data: &[u8]) {
        let p = root.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }
    fn game_fixture(root: &Path) {
        let mut pal = vec![0; 33536];
        pal[768..].fill(1);
        save(root, "COLOR.PAL", &pal);
        for color in 0..10 {
            save(root, &format!("{color}.RMP"), &[0; 259]);
        }
        let mut master = String::new();
        for arena in 0..11 {
            for stem in ["TILES", "XBRICK"] {
                let name = format!("{stem}{arena}.ANI");
                master.push_str(&format!("-{name}\n"));
                save(
                    root,
                    &format!("DATA/ANI/{name}"),
                    &formats::tests::animation_fixture(),
                );
            }
            save(
                root,
                &format!("DATA/RES/FIELD{arena}.PCX"),
                &formats::tests::pcx_fixture(),
            );
        }
        save(root, "DATA/ANI/MASTER.ALI", master.as_bytes());
        save(
            root,
            "DATA/SCHEMES/BASIC.SCH",
            formats::tests::scheme_fixture().as_bytes(),
        );
        save(root, "DATA/RES/SOUNDLST.RES", b"200,EXPLODE\n");
        save(root, "DATA/RES/VALUELST.RES", b"35,11\n");
        save(root, "MESSAGES.TXT", b"150,First arena\n");
        save(
            root,
            "DATA/SOUND/EXPLODE.RSS",
            &[0, 128, 255, 127, 255, 255],
        );
    }
    #[test]
    fn extracted_import_round_trip_checksums_and_corruption() {
        let t = Temp::new();
        let source = t.0.join("source");
        game_fixture(&source);
        let output = t.0.join("cache");
        let original = fs::read(source.join("COLOR.PAL")).unwrap();
        let m = import(&source, &output).unwrap();
        assert_eq!(m.arenas.len(), 11);
        assert_eq!(m.animations.len(), 22);
        assert_eq!(m.schemes.len(), 1);
        assert_eq!(m.sounds.len(), 1);
        assert_eq!(fs::read(source.join("COLOR.PAL")).unwrap(), original);
        assert!(m
            .warnings
            .iter()
            .any(|w| w.contains("missing right-channel")));
        assert_eq!(
            validate(&output).unwrap().generated.len(),
            m.generated.len()
        );
        let image = &m.generated[0];
        let path = output.join(&image.path);
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 1;
        fs::write(path, bytes).unwrap();
        assert!(validate(&output)
            .unwrap_err()
            .to_string()
            .contains("checksum"));
        assert!(import(&source, &output).is_err());
    }
    #[test]
    fn failed_import_is_not_published_and_source_cannot_be_output() {
        let t = Temp::new();
        let source = t.0.join("source");
        game_fixture(&source);
        let nested = source.join("must-not-be-created/cache");
        assert!(import(&source, &nested).is_err());
        assert!(!source.join("must-not-be-created").exists());
        save(&source, "DATA/ANI/TILES0.ANI", b"truncated");
        let output = t.0.join("cache");
        assert!(import(&source, &output).is_err());
        assert!(!output.exists());
        assert!(fs::read_dir(&t.0).unwrap().all(|p| !p
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".bomberman-import")));
    }
    #[test]
    fn manifest_rejects_paths_outside_cache_and_wrong_sprite_dimensions() {
        let t = Temp::new();
        let source = t.0.join("source");
        game_fixture(&source);
        let output = t.0.join("cache");
        let mut m = import(&source, &output).unwrap();
        m.generated[0].path = "../escape.png".into();
        assert!(check_manifest(&m).is_err());
        m.generated[0].path = "images/valid.png".into();
        let mut m = load_manifest(&output).unwrap();
        m.animations[0].frames[0].x = u32::MAX;
        assert!(check_manifest(&m).is_err());
        let mut m = load_manifest(&output).unwrap();
        m.textures[0].width += 1;
        fs::write(
            output.join("manifest.json"),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
        assert!(validate(&output)
            .unwrap_err()
            .to_string()
            .contains("dimensions"));
    }
}

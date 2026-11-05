//! Read-only original-format overrides, published as a separate validated cache.
use super::*;

/// Build or reuse a cache with `directory` taking precedence over `base`.
/// Missing mod assets retain the base asset; empty RSS files remain silent.
pub fn modded_cache(base: &Path, directory: &Path, cache_root: &Path) -> Result<PathBuf> {
    let base = fs::canonicalize(base).context("locate base asset cache")?;
    let mut manifest = validate(&base)?;
    let scheme_order: BTreeMap<_, _> = manifest
        .schemes
        .iter()
        .enumerate()
        .map(|(index, scheme)| (scheme.id.clone(), index))
        .collect();
    let mut source = source::Source::mod_directory(directory).context("open mod directory")?;
    let mut paths = BTreeMap::new();
    let mut records = BTreeMap::new();
    for path in source.paths() {
        let Some(id) = asset_id(&path, &manifest)? else {
            continue;
        };
        ensure!(
            paths.insert(id.clone(), path.clone()).is_none(),
            "duplicate mod override for {id}"
        );
        records.insert(id.clone(), source_record(&id, &source.read(&path)?));
    }
    ensure!(
        !paths.is_empty(),
        "mod directory has no supported original game assets"
    );
    let mut digest = Sha256::new();
    digest.update(b"atomic-bomberman-mod-cache-2\0");
    if records
        .keys()
        .any(|id| id == "COLOR.PAL" || id.ends_with(".RMP") || id == "DATA/RES/VALUELST.RES")
    {
        // Earlier color overlays discarded RMP footers. Rebuild their cheap
        // combined cache once while retaining the unchanged HD artwork key.
        digest.update(b"native-hud-colors-1\0");
    }
    digest.update(serde_json::to_vec(&manifest)?);
    for record in records.values() {
        digest.update(serde_json::to_vec(record)?);
    }
    let key = format!("{:x}", digest.finalize());
    ensure_safe_directory(cache_root)?;
    let cache_absolute = absolute_normalized(cache_root)?;
    let directory = fs::canonicalize(directory)?;
    ensure!(
        !cache_absolute.starts_with(&base) && !cache_absolute.starts_with(&directory),
        "mod cache must be outside the base and mod asset directories"
    );
    fs::create_dir_all(cache_root)?;
    let cache_root = fs::canonicalize(cache_root)?;
    let output = cache_root.join(key);
    if output.exists() {
        validate(&output).context("validate cached mod assets")?;
        return Ok(output);
    }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let stage = cache_root.join(format!(".mod-{}-{nonce}", std::process::id()));
    fs::create_dir(&stage)?;
    struct Stage(PathBuf);
    impl Drop for Stage {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let stage = Stage(stage);
    for generated in &manifest.generated {
        let target = stage.0.join(&generated.path);
        fs::create_dir_all(target.parent().context("asset has no parent")?)?;
        // Copy rather than link: replacing an override must never alter base data.
        fs::copy(base.join(&generated.path), target)?;
    }
    let changed = |id: &str| {
        records.get(id).is_some_and(|record| {
            !manifest
                .sources
                .iter()
                .any(|base| base.path == id && base.sha256 == record.sha256)
        })
    };
    let palette_changed = changed("COLOR.PAL");
    let remaps_changed = (0..10).any(|n| changed(&format!("{n}.RMP")));
    let fallback_colors_changed = changed("DATA/RES/VALUELST.RES")
        && manifest
            .sources
            .iter()
            .any(|s| s.path.ends_with(".RMP") && s.bytes == 256);
    let update_hud_colors = palette_changed || remaps_changed || fallback_colors_changed;
    let mut hud_profiles = hud::base_profiles(&manifest);
    if palette_changed {
        ensure!(manifest.animations.iter().all(|a| paths.contains_key(&a.id)),
            "a changed COLOR.PAL requires the mod's complete ANI set to preserve original color conversion");
    }
    let palette = if let Some(path) = paths.get("COLOR.PAL") {
        formats::Palette::parse(&read_checked(&mut source, path, &records["COLOR.PAL"])?)?
    } else {
        formats::Palette {
            rgb: manifest.palette.clone(),
            lookup: manifest.palette_lookup.clone(),
        }
    };
    let master = if let Some(path) = paths.get("DATA/ANI/MASTER.ALI") {
        formats::resource(
            "DATA/ANI/MASTER.ALI",
            &read_checked(&mut source, path, &records["DATA/ANI/MASTER.ALI"])?,
        )?
    } else {
        manifest
            .resources
            .iter()
            .find(|r| r.id == "DATA/ANI/MASTER.ALI")
            .context("base cache has no MASTER.ALI")?
            .clone()
    };
    let active = master
        .commands
        .iter()
        .filter(|c| c.kind.starts_with('-'))
        .map(|c| {
            format!(
                "DATA/ANI/{}",
                c.kind.trim_start_matches('-').to_ascii_uppercase()
            )
        })
        .collect::<BTreeSet<_>>();
    manifest.palette.clone_from(&palette.rgb);
    manifest.palette_lookup.clone_from(&palette.lookup);
    for n in 0..10 {
        if let Some(path) = paths.get(&format!("{n}.RMP")) {
            let data = read_checked(&mut source, path, &records[&format!("{n}.RMP")])?;
            ensure!(
                matches!(data.len(), 256 | 259),
                "{path}: expected 256 remap bytes, optionally followed by three color percentages"
            );
            manifest.player_remaps[n] = data[..256]
                .iter()
                .enumerate()
                .map(|(i, &v)| if v == 0 { i as u8 } else { v })
                .collect();
            hud_profiles[n] = hud::remap_profile(&data);
        }
    }
    for (id, path) in paths {
        let record = &records[&id];
        let same = manifest
            .sources
            .iter()
            .any(|old| old.path == id && old.sha256 == record.sha256);
        let animation = id.ends_with(".ANI");
        if same && !(animation && palette_changed) {
            continue;
        }
        let data = source.read(&path)?;
        ensure!(
            sha(&data) == record.sha256,
            "mod changed while preparing {path}; try again"
        );
        remove_asset(&mut manifest, &id);
        if id == "COLOR.PAL" || id.ends_with(".RMP") {
            manifest.sources.push(record.clone());
            continue;
        }
        if animation {
            ensure!(palette.lookup.len() == 32768,
                "animation overrides need COLOR.PAL with this older base cache; include the original COLOR.PAL in the mod or reimport the base game once");
        }
        decode_asset(&mut manifest, &palette, &active, &id, &data, &stage.0)
            .with_context(|| format!("apply mod asset {path}"))?;
    }
    if remaps_changed {
        rebuild_variants(&stage.0, &mut manifest, &palette)?;
    }
    manifest.textures.sort_by(|a, b| a.id.cmp(&b.id));
    manifest.animations.sort_by(|a, b| a.id.cmp(&b.id));
    manifest.fonts.sort_by(|a, b| a.id.cmp(&b.id));
    manifest.sounds.sort_by(|a, b| a.id.cmp(&b.id));
    // Saved settings use scheme indices. Keep base rows stable and append new
    // mod schemes, rather than silently selecting a different saved map.
    manifest.schemes.sort_by(|a, b| {
        scheme_order
            .get(&a.id)
            .unwrap_or(&usize::MAX)
            .cmp(scheme_order.get(&b.id).unwrap_or(&usize::MAX))
            .then_with(|| a.id.cmp(&b.id))
    });
    manifest.resources.sort_by(|a, b| a.id.cmp(&b.id));
    manifest.sources.sort_by(|a, b| a.path.cmp(&b.path));
    finish_manifest(&mut manifest)?;
    if update_hud_colors {
        hud::finish_profiles(&mut manifest, hud_profiles)?;
    }
    fs::write(
        stage.0.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    validate(&stage.0).context("validate combined mod assets")?;
    // Another instance can finish the same content-addressed cache first.
    if output.exists() {
        validate(&output)?;
    } else if let Err(error) = fs::rename(&stage.0, &output) {
        if output.is_dir() {
            validate(&output)?;
        } else {
            return Err(error).context("publish mod asset cache");
        }
    }
    Ok(output)
}

fn asset_id(path: &str, manifest: &Manifest) -> Result<Option<String>> {
    let ext = path.rsplit('.').next().unwrap_or_default();
    if !matches!(
        ext,
        "ANI" | "PCX" | "FON" | "RSS" | "SCH" | "BM" | "RMP" | "PAL" | "ALI" | "RES"
    ) && path != "MESSAGES.TXT"
    {
        return Ok(None);
    }
    let id = if manifest.sources.iter().any(|s| s.path == path) || path.starts_with("DATA/") {
        path.to_owned()
    } else if path.starts_with("SOUND/")
        || path.starts_with("ANI/")
        || path.starts_with("RES/")
        || path.starts_with("SCHEMES/")
    {
        format!("DATA/{path}")
    } else if path.contains('/') {
        return Ok(None);
    } else {
        let matches = manifest
            .sources
            .iter()
            .filter(|s| s.path.rsplit('/').next() == Some(path))
            .collect::<Vec<_>>();
        ensure!(
            matches.len() <= 1,
            "ambiguous flat mod filename {path}; use its DATA subdirectory"
        );
        if let Some(source) = matches.first() {
            source.path.clone()
        } else {
            match ext {
                "RSS" => format!("DATA/SOUND/{path}"),
                "ANI" => format!("DATA/ANI/{path}"),
                "PCX" => format!("DATA/RES/{path}"),
                "SCH" => format!("DATA/SCHEMES/{path}"),
                // BM95.RES and other Windows program resources are not assets.
                "RES" | "ALI" => return Ok(None),
                _ => path.into(),
            }
        }
    };
    if (ext == "PAL" && id != "COLOR.PAL")
        || (ext == "RMP" && !(0..10).any(|n| id == format!("{n}.RMP")))
        || (matches!(ext, "RES" | "ALI" | "BM" | "TXT") && !is_resource(&id))
    {
        return Ok(None);
    }
    Ok(Some(id))
}

fn remove_asset(manifest: &mut Manifest, id: &str) {
    let mut generated = BTreeSet::from([format!("tables/{id}.txt"), format!("unused/{id}")]);
    for a in manifest.animations.iter().filter(|a| a.id == id) {
        generated.insert(a.texture.clone());
        generated.insert(a.indexed_path.clone());
        generated.extend(a.player_variants.iter().cloned());
    }
    for a in manifest.textures.iter().filter(|a| a.id == id) {
        generated.insert(a.path.clone());
    }
    for a in manifest.fonts.iter().filter(|a| a.id == id) {
        generated.insert(a.texture.clone());
    }
    for a in manifest.sounds.iter().filter(|a| a.id == id) {
        generated.insert(a.path.clone());
    }
    manifest.animations.retain(|a| a.id != id);
    manifest.textures.retain(|a| a.id != id);
    manifest.fonts.retain(|a| a.id != id);
    manifest.sounds.retain(|a| a.id != id);
    manifest.schemes.retain(|a| a.id != id);
    manifest.resources.retain(|a| a.id != id);
    manifest.sources.retain(|a| a.path != id);
    manifest.generated.retain(|a| !generated.contains(&a.path));
    manifest.warnings.retain(|w| !w.starts_with(id));
}

fn rebuild_variants(
    cache: &Path,
    manifest: &mut Manifest,
    palette: &formats::Palette,
) -> Result<()> {
    for animation in &manifest.animations {
        if animation.player_variants.is_empty() {
            continue;
        }
        let decoder = png::Decoder::new(BufReader::new(File::open(
            cache.join(&animation.indexed_path),
        )?));
        let mut reader = decoder.read_info()?;
        let mut indices = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut indices)?;
        indices.truncate(info.buffer_size());
        for (index, path) in animation.player_variants.iter().enumerate() {
            manifest.generated.retain(|asset| &asset.path != path);
            write_png(
                cache,
                path,
                animation.width,
                animation.height,
                png::ColorType::Rgba,
                &palette.rgba(&indices, Some(&manifest.player_remaps[index]), true),
                &mut manifest.generated,
            )?;
        }
    }
    Ok(())
}

fn read_checked(
    source: &mut source::Source,
    path: &str,
    expected: &SourceAsset,
) -> Result<Vec<u8>> {
    let data = source.read(path)?;
    ensure!(
        sha(&data) == expected.sha256,
        "mod changed while preparing {path}; try again"
    );
    Ok(data)
}

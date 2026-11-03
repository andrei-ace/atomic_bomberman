use super::*;
use anyhow::ensure;
use image::{
    imageops::{self, FilterType},
    Rgba, RgbaImage,
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
pub struct Progress {
    pub message: String,
    pub completed: usize,
    pub total: usize,
}
impl Progress {
    pub(super) fn message(message: &str) -> Self {
        Self {
            message: message.into(),
            completed: 0,
            total: 0,
        }
    }
}
#[derive(Debug)]
pub(super) struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HD preparation cancelled")
    }
}
impl std::error::Error for Cancelled {}

pub(super) fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    Ok(())
}

#[derive(Clone)]
struct Piece {
    rect: [u32; 4],
    origin: [i32; 2],
}
struct Job {
    key: String,
    path: String,
    pieces: Vec<Piece>,
    remap: Option<usize>,
    font: bool,
    tile: bool,
}
fn jobs(m: &Manifest) -> Vec<Job> {
    let mut out = Vec::new();
    for texture in &m.textures {
        out.push(Job {
            key: texture.path.clone(),
            path: texture.path.clone(),
            pieces: vec![Piece {
                rect: [0, 0, texture.width, texture.height],
                origin: [0, 0],
            }],
            remap: None,
            font: false,
            tile: false,
        });
    }
    for font in &m.fonts {
        out.push(Job {
            key: font.texture.clone(),
            path: font.texture.clone(),
            pieces: font
                .glyphs
                .iter()
                .filter(|g| g.width > 0 && g.height > 0)
                .map(|g| Piece {
                    rect: [g.x, g.y, g.width, g.height],
                    origin: [0, 0],
                })
                .collect(),
            remap: None,
            font: true,
            tile: false,
        });
    }
    for animation in &m.animations {
        let pieces: Vec<_> = animation
            .frames
            .iter()
            .map(|f| Piece {
                rect: [f.x, f.y, f.width, f.height],
                origin: [f.origin_x, f.origin_y],
            })
            .collect();
        let tile = m.arenas.iter().any(|a| a.tiles == animation.id);
        out.push(Job {
            key: animation.texture.clone(),
            path: animation.texture.clone(),
            pieces: pieces.clone(),
            remap: None,
            font: false,
            tile,
        });
        if remapped(animation, m) {
            for variant in 0..m.player_remaps.len() {
                out.push(Job {
                    key: variant_key(animation, variant),
                    path: animation.indexed_path.clone(),
                    pieces: pieces.clone(),
                    remap: Some(variant),
                    font: false,
                    tile,
                });
            }
        }
    }
    out
}
pub(super) fn expected_images(m: &Manifest) -> BTreeMap<String, Vec<[u32; 4]>> {
    jobs(m)
        .into_iter()
        .map(|j| (j.key, j.pieces.into_iter().map(|p| p.rect).collect()))
        .collect()
}

struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs on a worker, never on the draw thread. The inference executable is a
/// separate cancellable process; original files are only read.
pub fn prepare(
    base: &Path,
    root: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(Progress) + Sync,
) -> Result<PathBuf> {
    prepare_with_workers(base, root, cancel, None, progress)
}

/// An explicit worker count is useful for reproducible performance checks. It
/// may reduce, but never exceed, the detected resource limit.
pub fn prepare_with_workers(
    base: &Path,
    root: &Path,
    cancel: &AtomicBool,
    requested_workers: Option<usize>,
    progress: impl Fn(Progress) + Sync,
) -> Result<PathBuf> {
    ensure!(
        requested_workers.is_none_or(|n| (1..=4).contains(&n)),
        "HD worker count must be between 1 and 4"
    );
    check_cancel(cancel)?;
    progress(Progress::message("Checking installed game data"));
    let manifest = crate::validate(base)?;
    crate::ensure_safe_directory(root)?;
    fs::create_dir_all(root)?;
    let base = fs::canonicalize(base)?;
    let root = fs::canonicalize(root)?;
    ensure!(
        !root.starts_with(&base),
        "HD cache must be outside the original asset cache"
    );
    let destination = cache_path(&root, &manifest)?;
    if Pack::load(&destination, &manifest).is_ok() {
        return Ok(destination);
    }
    let _lock = lock_cache(&root)?;
    if Pack::load(&destination, &manifest).is_ok() {
        return Ok(destination);
    }
    check_cancel(cancel)?;
    let tool = tool::Tool::obtain(&root, cancel, &progress)?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let stage = Stage(root.join(format!(".prepare-{}-{nonce}", std::process::id())));
    fs::create_dir(&stage.0)?;
    fs::create_dir(stage.0.join("pages"))?;
    let mut pack = Pack {
        version: VERSION,
        source: source_key(&manifest)?,
        scale: SCALE,
        model: MODEL.into(),
        images: BTreeMap::new(),
        pages: BTreeMap::new(),
    };
    let jobs = jobs(&manifest);
    let resources = Resources::detect();
    let workers = if let Some(requested) = requested_workers {
        ensure!(
            requested > 0 && requested <= resources.max_workers,
            "HD worker count must be between 1 and {} for the available resources",
            resources.max_workers
        );
        requested
    } else {
        calibrate(
            &base, &stage.0, &manifest, &jobs, &tool, &resources, cancel, &progress,
        )?
    };
    progress(Progress::message(&format!(
        "Preparing HD with {workers} parallel asset worker(s) ({} CPU threads)",
        resources.cpu_threads
    )));
    let total = jobs.iter().map(|j| j.pieces.len()).sum();
    let completed = Mutex::new(0);
    let counts: Vec<_> = jobs.iter().map(|_| AtomicUsize::new(0)).collect();
    scheduler::resilient(
        jobs.len(),
        workers,
        cancel,
        |index, stop, workers| {
            let job = &jobs[index];
            enhance_job(&base, &stage.0, &manifest, job, index, &tool, stop, |n| {
                let previous = counts[index].fetch_max(n, Ordering::AcqRel);
                let delta = n.saturating_sub(previous);
                let mut completed = completed.lock().unwrap_or_else(|e| e.into_inner());
                *completed += delta;
                progress(Progress {
                    message: format!("Enhancing {} ({workers} workers)", job.key),
                    completed: *completed,
                    total,
                });
            })
        },
        |index, output| {
            pack.images.insert(jobs[index].key.clone(), output.regions);
            pack.pages.extend(output.pages);
            Ok(())
        },
        |error| error.downcast_ref::<tool::InferenceFailure>().is_some(),
        || {
            progress(Progress {
            message:
                "GPU could not sustain parallel work; retrying unfinished assets with one worker"
                    .into(),
            completed: *completed.lock().unwrap_or_else(|e| e.into_inner()),
            total,
        })
        },
    )?;
    check_cancel(cancel)?;
    progress(Progress::message("Validating HD artwork"));
    fs::write(stage.0.join("hd.json"), serde_json::to_vec(&pack)?)?;
    Pack::load(&stage.0, &manifest)?;
    check_cancel(cancel)?;
    // A damaged old cache is retained until its replacement is fully validated.
    let old = root.join(format!(".old-{nonce}"));
    if destination.exists() {
        fs::rename(&destination, &old)?;
    }
    if let Err(error) = fs::rename(&stage.0, &destination) {
        if old.exists() {
            let _ = fs::rename(&old, &destination);
        }
        return Err(error.into());
    }
    let _ = fs::remove_dir_all(old);
    progress(Progress {
        message: "HD graphics ready".into(),
        completed: total,
        total,
    });
    Ok(destination)
}

struct JobOutput {
    regions: Vec<Region>,
    pages: BTreeMap<String, Page>,
}

#[allow(clippy::too_many_arguments)]
fn enhance_job(
    base: &Path,
    stage: &Path,
    manifest: &Manifest,
    job: &Job,
    index: usize,
    tool: &tool::Tool,
    cancel: &AtomicBool,
    progress: impl Fn(usize),
) -> Result<JobOutput> {
    check_cancel(cancel)?;
    progress(0);
    let work = Stage(stage.join(format!("work-{index}")));
    fs::create_dir(&work.0)?;
    let input = work.0.join("input");
    let output = work.0.join("output");
    fs::create_dir(&input)?;
    fs::create_dir(&output)?;
    let layout = aligned_canvas(&job.pieces)?;
    check_job_memory(base, job, layout)?;
    let image = read_job(base, job, manifest)?;
    let mut crops = Vec::new();
    let mut any_inference = false;
    for (i, piece) in job.pieces.iter().enumerate() {
        check_cancel(cancel)?;
        let [x, y, w, h] = piece.rect;
        ensure!(
            w > 0
                && h > 0
                && x.checked_add(w).is_some_and(|r| r <= image.width())
                && y.checked_add(h).is_some_and(|r| r <= image.height()),
            "Invalid source frame in {}",
            job.key
        );
        let original = imageops::crop_imm(&image, x, y, w, h).to_image();
        let blank = original.pixels().all(|p| p[3] == 0);
        let offset = if job.pieces.len() == 1 && !job.font {
            [0, 0]
        } else {
            [
                (layout.0 - piece.origin[0]) as u32,
                (layout.1 - piece.origin[1]) as u32,
            ]
        };
        if job.font || blank {
            // Fonts keep the exact original characters and metrics. Only
            // their coverage is reconstructed; AI never guesses letters.
            let enlarged =
                imageops::resize(&original, w * SCALE, h * SCALE, FilterType::CatmullRom);
            enlarged.save(output.join(format!("{i:04}.png")))?;
        } else {
            let mut canvas = if job.pieces.len() == 1 {
                original.clone()
            } else {
                RgbaImage::new(layout.2, layout.3)
            };
            if job.tile {
                repeat_context(&mut canvas, &original, offset);
            } else {
                imageops::replace(&mut canvas, &original, offset[0] as i64, offset[1] as i64);
                dilate_rgb(&mut canvas);
            }
            canvas.save(input.join(format!("{i:04}.png")))?;
            any_inference = true;
        }
        crops.push((original, offset, blank));
    }
    if any_inference {
        tool.run(&input, &output, cancel)?;
    }
    let mut atlas = Atlas::new(stage, index);
    let mut pages = BTreeMap::new();
    let mut regions = Vec::new();
    for (i, (original, offset, blank)) in crops.into_iter().enumerate() {
        check_cancel(cancel)?;
        let path = output.join(format!("{i:04}.png"));
        let mut enhanced = image::open(&path)
            .with_context(|| format!("HD upscaler did not produce frame {i} of {}", job.key))?
            .into_rgba8();
        let w = original.width() * SCALE;
        let h = original.height() * SCALE;
        if !job.font && !blank {
            let expected = if job.pieces.len() == 1 {
                (w, h)
            } else {
                (layout.2 * SCALE, layout.3 * SCALE)
            };
            ensure!(
                enhanced.dimensions() == expected,
                "HD upscaler returned unexpected frame dimensions"
            );
            enhanced = imageops::crop_imm(&enhanced, offset[0] * SCALE, offset[1] * SCALE, w, h)
                .to_image();
            let alpha = imageops::resize(&original, w, h, FilterType::CatmullRom);
            for (pixel, source) in enhanced.pixels_mut().zip(alpha.pixels()) {
                pixel[3] = source[3];
            }
        }
        ensure!(
            enhanced.dimensions() == (w, h),
            "HD frame dimensions do not match the original"
        );
        let (page, rect) = atlas.push(&enhanced, &mut pages)?;
        regions.push(Region {
            original: job.pieces[i].rect,
            page,
            rect,
        });
        progress(i + 1);
    }
    atlas.flush(&mut pages)?;
    Ok(JobOutput { regions, pages })
}

fn check_job_memory(base: &Path, job: &Job, layout: (i32, i32, u32, u32)) -> Result<()> {
    let (w, h) = image::image_dimensions(base.join(&job.path))?;
    let retained: u64 = job
        .pieces
        .iter()
        .map(|p| u64::from(p.rect[2]) * u64::from(p.rect[3]) * 4)
        .sum();
    // Include decode/remap overlap, retained original crops, enhanced canvases,
    // alpha reconstruction and atlas buffers; leave the rest for the helper.
    let estimate = u64::from(w) * u64::from(h) * 8
        + retained
        + u64::from(layout.2) * u64::from(layout.3) * 4 * (SCALE * SCALE * 3 + 3) as u64
        + u64::from(PAGE_SIZE) * u64::from(PAGE_SIZE) * 8;
    ensure!(
        w <= 8192 && h <= 8192 && estimate <= 768 * 1024 * 1024,
        "{} exceeds the HD worker memory budget; Classic graphics remain available",
        job.key
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn calibrate(
    base: &Path,
    stage: &Path,
    manifest: &Manifest,
    jobs: &[Job],
    tool: &tool::Tool,
    resources: &Resources,
    cancel: &AtomicBool,
    progress: &impl Fn(Progress),
) -> Result<usize> {
    let limit = resources.max_workers.min(jobs.len().max(1));
    let candidate = jobs
        .iter()
        .filter(|j| !j.font && !j.tile && j.remap.is_none() && j.pieces.len() >= 16)
        .filter_map(|j| aligned_canvas(&j.pieces).ok().map(|layout| (j, layout)))
        .filter(|(_, layout)| layout.2 <= 256 && layout.3 <= 256)
        .min_by_key(|(_, layout)| (layout.2 * layout.3).abs_diff(128 * 128));
    let Some((job, layout)) = candidate.filter(|_| limit > 1) else {
        return Ok(1);
    };
    check_job_memory(base, job, layout)?;
    let source = read_job(base, job, manifest)?;
    let work = Stage(stage.join("calibration"));
    fs::create_dir(&work.0)?;
    let samples = work.0.join("samples");
    fs::create_dir(&samples)?;
    const SAMPLES: usize = 24;
    for i in 0..SAMPLES {
        check_cancel(cancel)?;
        let piece = &job.pieces[i % job.pieces.len()];
        let [x, y, w, h] = piece.rect;
        ensure!(
            x.checked_add(w).is_some_and(|r| r <= source.width())
                && y.checked_add(h).is_some_and(|r| r <= source.height()),
            "Invalid HD calibration source frame"
        );
        let original = imageops::crop_imm(&source, x, y, w, h).to_image();
        let mut canvas = RgbaImage::new(layout.2, layout.3);
        imageops::replace(
            &mut canvas,
            &original,
            i64::from(layout.0 - piece.origin[0]),
            i64::from(layout.1 - piece.origin[1]),
        );
        dilate_rgb(&mut canvas);
        canvas.save(samples.join(format!("{i:04}.png")))?;
    }
    drop(source);
    let mut selected = 1;
    let mut best = f64::INFINITY;
    let mut timings = Vec::new();
    let mut candidates = vec![1, 2, limit];
    candidates.sort_unstable();
    candidates.dedup();
    for workers in candidates {
        progress(Progress::message(&format!(
            "Calibrating HD performance: {workers} parallel worker(s)"
        )));
        let trial = work.0.join(format!("workers-{workers}"));
        fs::create_dir(&trial)?;
        for worker in 0..workers {
            let input = trial.join(format!("input-{worker}"));
            fs::create_dir(&input)?;
            fs::create_dir(trial.join(format!("output-{worker}")))?;
            for i in (worker..SAMPLES).step_by(workers) {
                let name = format!("{i:04}.png");
                fs::copy(samples.join(&name), input.join(&name))?;
            }
        }
        let start = std::time::Instant::now();
        let result = scheduler::run(
            workers,
            workers,
            cancel,
            |worker, stop| {
                tool.run(
                    &trial.join(format!("input-{worker}")),
                    &trial.join(format!("output-{worker}")),
                    stop,
                )
            },
            |_, ()| Ok(()),
        );
        if let Err(error) = result {
            check_cancel(cancel)?;
            if workers == 1 || error.downcast_ref::<tool::InferenceFailure>().is_none() {
                return Err(error);
            }
            // A successful serial trial is already available. Do not probe a
            // still larger pool after this GPU rejected the smaller one.
            selected = 1;
            break;
        }
        let seconds = start.elapsed().as_secs_f64();
        timings.push(serde_json::json!({"workers":workers,"seconds":seconds,"frames":SAMPLES}));
        // Prefer fewer contexts when the measured difference is within noise.
        if seconds < best * 0.95 {
            best = seconds;
            selected = workers;
        }
    }
    fs::write(
        stage.join("preparation.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "resources":resources, "selected_workers":selected, "calibration":timings,
        }))?,
    )?;
    Ok(selected)
}

fn read_job(base: &Path, job: &Job, m: &Manifest) -> Result<RgbaImage> {
    let image = image::open(base.join(&job.path))?;
    if let Some(variant) = job.remap {
        let indices = image.into_luma8();
        let mut rgba = RgbaImage::new(indices.width(), indices.height());
        let remap = m
            .player_remaps
            .get(variant)
            .context("Missing player color remap")?;
        ensure!(
            remap.len() == 256 && m.palette.len() == 256,
            "Invalid source palette"
        );
        for (pixel, index) in rgba.pixels_mut().zip(indices.pixels()) {
            let i = index[0] as usize;
            let rgb = m.palette[remap[i] as usize];
            *pixel = Rgba([rgb[0], rgb[1], rgb[2], if i == 0 { 0 } else { 255 }]);
        }
        Ok(rgba)
    } else {
        Ok(image.into_rgba8())
    }
}

fn aligned_canvas(pieces: &[Piece]) -> Result<(i32, i32, u32, u32)> {
    ensure!(!pieces.is_empty(), "Empty HD animation");
    ensure!(
        pieces.iter().all(|p| p.rect[2] > 0
            && p.rect[3] > 0
            && p.rect[2] <= 1024
            && p.rect[3] <= 1024
            && p.origin.iter().all(|v| (-16384..=16384).contains(v))),
        "HD source frame exceeds limits"
    );
    let left = pieces.iter().map(|p| p.origin[0]).max().unwrap() + 16;
    let top = pieces.iter().map(|p| p.origin[1]).max().unwrap() + 16;
    let right = pieces
        .iter()
        .map(|p| p.rect[2] as i32 - p.origin[0])
        .max()
        .unwrap()
        + 16;
    let bottom = pieces
        .iter()
        .map(|p| p.rect[3] as i32 - p.origin[1])
        .max()
        .unwrap()
        + 16;
    let w = left
        .checked_add(right)
        .context("HD canvas width overflow")?;
    let h = top
        .checked_add(bottom)
        .context("HD canvas height overflow")?;
    ensure!(
        w > 0 && h > 0 && w <= 1024 && h <= 1024,
        "HD source canvas exceeds limits"
    );
    Ok((left, top, w as u32, h as u32))
}

fn repeat_context(canvas: &mut RgbaImage, source: &RgbaImage, offset: [u32; 2]) {
    for (x, y, pixel) in canvas.enumerate_pixels_mut() {
        let sx = (x as i64 - offset[0] as i64).rem_euclid(source.width() as i64) as u32;
        let sy = (y as i64 - offset[1] as i64).rem_euclid(source.height() as i64) as u32;
        *pixel = *source.get_pixel(sx, sy);
    }
}

fn dilate_rgb(image: &mut RgbaImage) {
    for _ in 0..4 {
        let original = image.clone();
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            if pixel[3] != 0 {
                continue;
            }
            for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 0
                    || ny < 0
                    || nx >= original.width() as i64
                    || ny >= original.height() as i64
                {
                    continue;
                }
                let p = original.get_pixel(nx as u32, ny as u32);
                if p[3] != 0 {
                    pixel[0] = p[0];
                    pixel[1] = p[1];
                    pixel[2] = p[2];
                    break;
                }
            }
        }
    }
}

struct Atlas<'a> {
    root: &'a Path,
    group: usize,
    page: usize,
    image: RgbaImage,
    x: u32,
    y: u32,
    row: u32,
    used: u32,
}
impl<'a> Atlas<'a> {
    fn new(root: &'a Path, group: usize) -> Self {
        Self {
            root,
            group,
            page: 0,
            image: RgbaImage::new(PAGE_SIZE, PAGE_SIZE),
            x: 4,
            y: 4,
            row: 0,
            used: 0,
        }
    }
    fn name(&self) -> String {
        format!("pages/{:04}-{:03}.png", self.group, self.page)
    }
    fn push(
        &mut self,
        image: &RgbaImage,
        pages: &mut BTreeMap<String, Page>,
    ) -> Result<(String, [u32; 4])> {
        let (w, h) = image.dimensions();
        if w + 8 > PAGE_SIZE || h + 8 > PAGE_SIZE {
            self.flush(pages)?;
            ensure!(
                w <= 4096 && h <= 4096,
                "HD image exceeds texture size limit"
            );
            let name = self.name();
            self.write(&name, image, pages)?;
            self.page += 1;
            return Ok((name, [0, 0, w, h]));
        }
        if self.x + w + 4 > PAGE_SIZE {
            self.x = 4;
            self.y += self.row + 8;
            self.row = 0;
        }
        if self.y + h + 4 > PAGE_SIZE {
            self.flush(pages)?;
        }
        let rect = [self.x, self.y, w, h];
        // Extrude edge texels into the atlas gutter for linear filtering.
        for y in 0..h + 8 {
            for x in 0..w + 8 {
                self.image.put_pixel(
                    self.x + x - 4,
                    self.y + y - 4,
                    *image.get_pixel(
                        x.saturating_sub(4).min(w - 1),
                        y.saturating_sub(4).min(h - 1),
                    ),
                );
            }
        }
        self.x += w + 8;
        self.row = self.row.max(h);
        self.used = self.used.max(self.y + h + 4);
        Ok((self.name(), rect))
    }
    fn write(
        &self,
        name: &str,
        image: &RgbaImage,
        pages: &mut BTreeMap<String, Page>,
    ) -> Result<()> {
        let path = self.root.join(name);
        image.save(&path)?;
        let (bytes, sha256) = file_hash(&path)?;
        pages.insert(
            name.into(),
            Page {
                width: image.width(),
                height: image.height(),
                bytes,
                sha256,
            },
        );
        Ok(())
    }
    fn flush(&mut self, pages: &mut BTreeMap<String, Page>) -> Result<()> {
        if self.used == 0 {
            return Ok(());
        }
        let height = self.used.next_power_of_two().min(PAGE_SIZE);
        let image = imageops::crop_imm(&self.image, 0, 0, PAGE_SIZE, height).to_image();
        self.write(&self.name(), &image, pages)?;
        self.page += 1;
        self.image = RgbaImage::new(PAGE_SIZE, PAGE_SIZE);
        self.x = 4;
        self.y = 4;
        self.row = 0;
        self.used = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canvases_preserve_different_frame_origins() {
        let pieces = vec![
            Piece {
                rect: [3, 4, 20, 30],
                origin: [10, 29],
            },
            Piece {
                rect: [8, 9, 25, 20],
                origin: [12, 18],
            },
        ];
        let (left, top, w, h) = aligned_canvas(&pieces).unwrap();
        for p in pieces {
            assert!(left - p.origin[0] >= 16);
            assert!(top - p.origin[1] >= 16);
            assert!(left - p.origin[0] + p.rect[2] as i32 <= w as i32 - 16);
            assert!(top - p.origin[1] + p.rect[3] as i32 <= h as i32 - 16);
        }
    }
    #[test]
    fn cancelled_preparation_never_touches_source_or_creates_a_cache() {
        let cancelled = AtomicBool::new(true);
        assert!(prepare(
            Path::new("missing-source"),
            Path::new("missing-output"),
            &cancelled,
            |_| {}
        )
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    }
    #[test]
    fn tile_context_wraps_around_frame_edges() {
        let source = RgbaImage::from_fn(2, 2, |x, y| Rgba([x as u8, y as u8, 255, 255]));
        let mut canvas = RgbaImage::new(6, 6);
        repeat_context(&mut canvas, &source, [2, 2]);
        assert_eq!(canvas.get_pixel(1, 2), source.get_pixel(1, 0));
        assert_eq!(canvas.get_pixel(4, 5), source.get_pixel(0, 1));
    }

    /// Explicit hardware check: actual original sprites/palettes/tiles/fonts
    /// and the pinned real model, with all output confined to a private stage.
    #[test]
    #[ignore = "requires local original assets, the pinned helper and a Vulkan GPU"]
    fn real_parallel_enhancement_matches_serial_pages() -> Result<()> {
        let path = |name| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .with_context(|| format!("Set {name}"))
        };
        let base = path("ATOMIC_BOMBERMAN_HD_BENCH_ASSETS")?;
        let cache = path("ATOMIC_BOMBERMAN_HD_BENCH_CACHE")?;
        let output = path("ATOMIC_BOMBERMAN_HD_BENCH_OUTPUT")?;
        fs::create_dir_all(&output)?;
        let cancel = AtomicBool::new(false);
        let manifest = crate::validate(&base)?;
        let tool = tool::Tool::obtain(&cache, &cancel, &|_| {})?;
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let stage = Stage(output.join(format!("stage-{nonce}")));
        fs::create_dir(&stage.0)?;
        let all = jobs(&manifest);
        let walk = manifest
            .animation("DATA/ANI/WALK.ANI")
            .context("Walking artwork required")?;
        let mut sample: Vec<_> = all
            .into_iter()
            .filter(|j| j.path == walk.texture || j.path == walk.indexed_path)
            .take(4)
            .collect();
        for job in &mut sample {
            job.pieces.truncate(16);
        }
        let mut other = jobs(&manifest);
        for wanted in ["DATA/ANI/TRIGANIM.ANI", &manifest.arenas[0].tiles] {
            let texture = &manifest
                .animation(wanted)
                .context("Sample animation required")?
                .texture;
            let index = other
                .iter()
                .position(|j| j.key == *texture)
                .context("Missing sample animation")?;
            let mut job = other.remove(index);
            job.pieces.truncate(24);
            sample.push(job);
        }
        let mut font = other
            .into_iter()
            .find(|j| j.font)
            .context("Sample font required")?;
        font.pieces.truncate(16);
        sample.push(font);
        let resources = Resources::detect();
        ensure!(
            resources.max_workers >= 2,
            "Parallel hardware comparison requires capacity for two workers"
        );
        let workers = calibrate(
            &base,
            &stage.0,
            &manifest,
            &sample,
            &tool,
            &resources,
            &cancel,
            &|p| eprintln!("{}", p.message),
        )?;
        let mut baseline = BTreeMap::new();
        let mut timings = Vec::new();
        for (label, concurrency) in [
            ("serial", 1),
            ("parallel", workers.max(2).min(resources.max_workers)),
        ] {
            let root = stage.0.join(label);
            fs::create_dir(&root)?;
            fs::create_dir(root.join("pages"))?;
            let start = std::time::Instant::now();
            scheduler::run(
                sample.len(),
                concurrency,
                &cancel,
                |index, stop| {
                    enhance_job(
                        &base,
                        &root,
                        &manifest,
                        &sample[index],
                        index,
                        &tool,
                        stop,
                        |_| {},
                    )
                },
                |index, output| {
                    let signature = serde_json::to_vec(&(output.regions, output.pages))?;
                    if label == "serial" {
                        baseline.insert(index, signature);
                    } else {
                        ensure!(
                            baseline.get(&index) == Some(&signature),
                            "Parallel output differs for {}",
                            sample[index].key
                        );
                    }
                    Ok(())
                },
            )?;
            timings.push(serde_json::json!({"mode":label,"workers":concurrency,"seconds":start.elapsed().as_secs_f64()}));
        }
        let report = serde_json::json!({"passed":true,"resources":resources,"calibrated_workers":workers,
            "calibration":serde_json::from_slice::<serde_json::Value>(&fs::read(stage.0.join("preparation.json"))?)?,
            "timings":timings,"groups":sample.len(),"frames":sample.iter().map(|j|j.pieces.len()).sum::<usize>(),
            "all_page_hashes_and_region_metadata_equal":true,
            "scope":"Real model on a representative subset of original walking colors, menu bomb, tiles and fonts; not a full-pack timing."});
        fs::write(
            output.join("report.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        eprintln!("{}", serde_json::to_string_pretty(&report)?);
        Ok(())
    }
}

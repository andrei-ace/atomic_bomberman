//! Bounded, asynchronous display pages; PNG decoding never occupies the frame
//! loop. Original textures remain usable while pages arrive.
use anyhow::{ensure, Context, Result};
use bomberman_assets::hd::{Pack, PageDecoder, Resources};
use macroquad::prelude::*;
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
};

type Key = (String, bool);
struct Cached {
    texture: Texture2D,
    bytes: u64,
    last_frame: u64,
}
struct Loaded {
    key: Key,
    image: Result<Image>,
}
struct Active {
    pack: Pack,
    decoder: Arc<PageDecoder>,
    textures: HashMap<Key, Cached>,
    requests: BTreeSet<Key>,
    pending: VecDeque<Key>,
    queued: BTreeSet<Key>,
    in_flight: BTreeSet<Key>,
    ready: mpsc::Receiver<Loaded>,
    sender: mpsc::Sender<Loaded>,
    cancel: Arc<AtomicBool>,
    workers: usize,
    budget: u64,
    bytes: u64,
    frame: u64,
    loads: u64,
    evictions: u64,
    hd_draws: u64,
    waiting_draws: u64,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub(super) struct Graphics {
    active: RefCell<Option<Active>>,
}

impl Graphics {
    pub(super) fn enabled(&self) -> bool {
        self.active.borrow().is_some()
    }
    pub(super) fn disable(&self) {
        if self.enabled() {
            unsafe {
                macroquad::window::get_internal_gl().flush();
            }
        }
        self.active.replace(None);
    }
    pub(super) fn activate(&self, root: PathBuf, pack: Pack) -> Result<()> {
        let limit = texture_limit();
        ensure!(
            pack.pages
                .values()
                .all(|p| p.width <= limit && p.height <= limit),
            "The display does not support the HD texture sizes"
        );
        let decoder = Arc::new(PageDecoder::new(&root, &pack));
        let capacity = Resources::detect();
        let (sender, ready) = mpsc::channel();
        self.active.replace(Some(Active {
            pack,
            decoder,
            textures: HashMap::new(),
            requests: BTreeSet::new(),
            pending: VecDeque::new(),
            queued: BTreeSet::new(),
            in_flight: BTreeSet::new(),
            ready,
            sender,
            cancel: Arc::new(AtomicBool::new(false)),
            workers: capacity.max_workers.clamp(1, 2),
            budget: capacity.texture_cache_bytes(),
            bytes: 0,
            frame: 0,
            loads: 0,
            evictions: 0,
            hd_draws: 0,
            waiting_draws: 0,
        }));
        Ok(())
    }
    pub(super) fn begin_frame(&self) {
        if let Some(active) = &mut *self.active.borrow_mut() {
            active.frame += 1;
        }
    }
    pub(super) fn bytes(&self) -> u64 {
        self.active.borrow().as_ref().map_or(0, |a| a.bytes)
    }
    pub(super) fn ready(&self) -> bool {
        self.active.borrow().as_ref().is_some_and(|a| {
            a.bytes > 0 && a.requests.is_empty() && a.pending.is_empty() && a.in_flight.is_empty()
        })
    }
    #[cfg(debug_assertions)]
    pub(super) fn diagnostics(&self) -> serde_json::Value {
        self.active.borrow().as_ref().map_or(serde_json::Value::Null, |a| serde_json::json!({
            "budget_bytes": a.budget, "resident_bytes": a.bytes, "pages": a.textures.len(),
            "loads": a.loads, "evictions": a.evictions, "in_flight": a.in_flight.len(),
            "pending": a.pending.len(), "hd_draws": a.hd_draws, "waiting_draws": a.waiting_draws,
        }))
    }
    pub(super) fn draw(
        &self,
        key: &str,
        source: Rect,
        destination: Rect,
        color: Color,
        keyed: bool,
    ) -> bool {
        if source.w <= 0. || source.h <= 0. {
            return false;
        }
        let mut state = self.active.borrow_mut();
        let Some(active) = state.as_mut() else {
            return false;
        };
        let Some(regions) = active.pack.images.get(key) else {
            return false;
        };
        let Some(region) = regions.iter().find(|r| {
            let [x, y, w, h] = r.original;
            source.x >= x as f32
                && source.y >= y as f32
                && source.x + source.w <= (x + w) as f32 + 0.01
                && source.y + source.h <= (y + h) as f32 + 0.01
        }) else {
            return false;
        };
        let [sx, sy, _, _] = region.original;
        let [x, y, _, _] = region.rect;
        let scale = active.pack.scale as f32;
        let source = Rect::new(
            x as f32 + (source.x - sx as f32) * scale,
            y as f32 + (source.y - sy as f32) * scale,
            source.w * scale,
            source.h * scale,
        );
        let texture_key = (region.page.clone(), keyed);
        let Some(cached) = active.textures.get_mut(&texture_key) else {
            active.requests.insert(texture_key);
            active.waiting_draws += 1;
            return false;
        };
        cached.last_frame = active.frame;
        active.hd_draws += 1;
        draw_texture_ex(
            &cached.texture,
            destination.x,
            destination.y,
            color,
            DrawTextureParams {
                source: Some(source),
                dest_size: Some(vec2(destination.w, destination.h)),
                ..Default::default()
            },
        );
        true
    }
    pub(super) async fn load_requested(&self) -> Result<()> {
        let mut state = self.active.borrow_mut();
        let Some(active) = state.as_mut() else {
            return Ok(());
        };
        // Upload at most one ready page per frame; CPU decoding and filtering
        // are bounded separately, with at most two completed buffers in flight.
        if let Ok(loaded) = active.ready.try_recv() {
            active.in_flight.remove(&loaded.key);
            let image = loaded.image?;
            let required = u64::from(image.width) * u64::from(image.height) * 4;
            while active.bytes + required > active.budget {
                let victim = active
                    .textures
                    .iter()
                    .filter(|(_, p)| p.last_frame + 2 < active.frame)
                    .min_by_key(|(_, p)| p.last_frame)
                    .map(|(name, _)| name.clone());
                let Some(victim) = victim else {
                    anyhow::bail!("HD artwork exceeds the available texture budget");
                };
                if let Some(old) = active.textures.remove(&victim) {
                    active.bytes -= old.bytes;
                    active.evictions += 1;
                }
            }
            let texture = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Texture2D::from_image(&image)
            }))
            .map_err(|_| anyhow::anyhow!("The display could not allocate an HD texture"))?;
            check_gl_error()?;
            texture.set_filter(FilterMode::Linear);
            active.textures.insert(
                loaded.key,
                Cached {
                    texture,
                    bytes: required,
                    last_frame: active.frame,
                },
            );
            active.bytes += required;
            active.loads += 1;
        }
        for key in std::mem::take(&mut active.requests) {
            if !active.textures.contains_key(&key)
                && !active.in_flight.contains(&key)
                && active.queued.insert(key.clone())
            {
                ensure!(
                    active.pending.len() < 256,
                    "Too many pending HD display pages"
                );
                active.pending.push_back(key);
            }
        }
        while active.in_flight.len() < active.workers {
            let Some(key) = active.pending.pop_front() else {
                break;
            };
            active.queued.remove(&key);
            if active.textures.contains_key(&key) {
                continue;
            }
            active.in_flight.insert(key.clone());
            let decoder = active.decoder.clone();
            let cancel = active.cancel.clone();
            let sender = active.sender.clone();
            std::thread::Builder::new()
                .name("HD display page".into())
                .spawn(move || {
                    let image = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        decoder.decode(&key.0, key.1, &cancel).map(|image| Image {
                            width: image.width() as u16,
                            height: image.height() as u16,
                            bytes: image.into_raw(),
                        })
                    }))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Could not decode HD artwork")));
                    if !cancel.load(Ordering::Acquire) {
                        let _ = sender.send(Loaded { key, image });
                    }
                })
                .context("Could not start HD display worker")?;
        }
        Ok(())
    }
}

pub(crate) fn texture_limit() -> u32 {
    let gl = unsafe { macroquad::window::get_internal_gl() };
    if gl.quad_context.info().backend == macroquad::miniquad::Backend::OpenGl {
        let mut limit = 0;
        unsafe {
            macroquad::miniquad::gl::glGetIntegerv(
                macroquad::miniquad::gl::GL_MAX_TEXTURE_SIZE,
                &mut limit,
            )
        };
        (limit.max(0) as u32).min(8192)
    } else {
        8192
    }
}
pub(crate) fn check_gl_error() -> Result<()> {
    let gl = unsafe { macroquad::window::get_internal_gl() };
    if gl.quad_context.info().backend == macroquad::miniquad::Backend::OpenGl {
        let error = unsafe { macroquad::miniquad::gl::glGetError() };
        ensure!(
            error == 0,
            "The graphics driver could not allocate HD resources (0x{error:x})"
        );
    }
    Ok(())
}
pub(crate) fn target(width: u32, height: u32) -> Result<RenderTarget> {
    let limit = texture_limit();
    ensure!(
        width > 0
            && height > 0
            && width <= limit
            && height <= limit
            && width as u64 * height as u64 <= 16 * 1024 * 1024,
        "The display resolution exceeds supported HD rendering limits"
    );
    let target = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        render_target(width, height)
    }))
    .map_err(|_| anyhow::anyhow!("The display could not allocate the HD frame"))?;
    check_gl_error()?;
    target.texture.set_filter(FilterMode::Linear);
    Ok(target)
}

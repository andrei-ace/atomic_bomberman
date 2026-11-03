//! GPU resources for the optional HD overlay. Original textures remain available
//! throughout activation and recovery, so the simulation can keep running.
use anyhow::{ensure, Context, Result};
use bomberman_assets::hd::Pack;
use macroquad::prelude::*;
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    path::PathBuf,
};

const MAX_TEXTURE_BYTES: u64 = 512 * 1024 * 1024;
struct Cached {
    texture: Texture2D,
    bytes: u64,
    last_frame: u64,
}
struct Active {
    root: PathBuf,
    pack: Pack,
    textures: HashMap<(String, bool), Cached>,
    requests: BTreeSet<(String, bool)>,
    bytes: u64,
    frame: u64,
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
            // A recoverable page failure can arrive after draw calls were
            // queued. Submit them before releasing textures used by that batch.
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
        self.active.replace(Some(Active {
            root,
            pack,
            textures: HashMap::new(),
            requests: BTreeSet::new(),
            bytes: 0,
            frame: 0,
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
        let name = region.page.clone();
        let [sx, sy, _, _] = region.original;
        let [x, y, _, _] = region.rect;
        let scale = active.pack.scale as f32;
        let source = Rect::new(
            x as f32 + (source.x - sx as f32) * scale,
            y as f32 + (source.y - sy as f32) * scale,
            source.w * scale,
            source.h * scale,
        );
        let frame = active.frame;
        let texture_key = (name, keyed);
        let Some(cached) = active.textures.get_mut(&texture_key) else {
            active.requests.insert(texture_key);
            return false;
        };
        cached.last_frame = frame;
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
        let requests = match &mut *self.active.borrow_mut() {
            Some(a) => std::mem::take(&mut a.requests),
            None => return Ok(()),
        };
        for (name, keyed) in requests {
            let (path, expected) = {
                let mut state = self.active.borrow_mut();
                let Some(active) = state.as_mut() else {
                    return Ok(());
                };
                let page = active.pack.pages.get(&name).context("Missing HD page")?;
                let required = page.width as u64 * page.height as u64 * 4;
                while active.bytes + required > MAX_TEXTURE_BYTES {
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
                    }
                }
                (
                    active.root.join(&name),
                    (page.width, page.height, page.bytes, page.sha256.clone()),
                )
            };
            // File hashes were checked by the worker; check again on first load
            // to handle a cache being changed or removed while the game runs.
            let data = load_file(&path.to_string_lossy())
                .await
                .context("read HD artwork")?;
            use sha2::{Digest, Sha256};
            ensure!(
                data.len() as u64 == expected.2
                    && format!("{:x}", Sha256::digest(&data)) == expected.3,
                "HD artwork is damaged"
            );
            let mut image = Image::from_file_with_format(&data, Some(ImageFormat::Png))
                .context("decode HD artwork")?;
            if keyed {
                for pixel in image.bytes.chunks_exact_mut(4) {
                    if pixel[..3] == [0, 0, 0] {
                        pixel[3] = 0;
                    }
                }
            }
            ensure!(
                (image.width as u32, image.height as u32) == (expected.0, expected.1),
                "HD texture dimensions changed"
            );
            let texture = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Texture2D::from_image(&image)
            }))
            .map_err(|_| anyhow::anyhow!("The display could not allocate an HD texture"))?;
            check_gl_error()?;
            texture.set_filter(FilterMode::Linear);
            let mut state = self.active.borrow_mut();
            let Some(active) = state.as_mut() else {
                return Ok(());
            };
            let bytes = expected.0 as u64 * expected.1 as u64 * 4;
            active.textures.insert(
                (name, keyed),
                Cached {
                    texture,
                    bytes,
                    last_frame: active.frame,
                },
            );
            active.bytes += bytes;
        }
        // Old arena and menu pages can be reclaimed even below the hard limit.
        if let Some(active) = self.active.borrow_mut().as_mut() {
            let frame = active.frame;
            active.textures.retain(|_, p| {
                let keep = p.last_frame + 300 >= frame;
                if !keep {
                    active.bytes -= p.bytes;
                }
                keep
            });
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

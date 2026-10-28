use anyhow::{Context, Result};
use bomberman_assets::{AnimationAsset, AnimationFrame, AnimationSequence, FontAsset, Manifest};
use bomberman_core::{GameEvent, Player};
use macroquad::{
    audio::{load_sound, play_sound, set_sound_volume, stop_sound, PlaySoundParams, Sound},
    prelude::*,
};
use std::{collections::HashMap, path::PathBuf};

pub struct Media {
    pub manifest: Manifest,
    cache: PathBuf,
    textures: HashMap<String, Texture2D>,
    runtime_variants: HashMap<String, Vec<Texture2D>>,
    sequences: HashMap<String, (usize, usize)>,
    sounds: HashMap<String, Sound>,
    music: Option<String>,
    font: Option<FontAsset>,
    pub warnings: Vec<String>,
    pub mute: bool,
    pub volume: f32,
    effect_limit: i32,
}

fn basename(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}

impl Media {
    pub async fn load(cache: PathBuf, manifest: Manifest, mute: bool, volume: f32) -> Result<Self> {
        let font = manifest
            .fonts
            .iter()
            .min_by_key(|font| font.line_height.abs_diff(16))
            .cloned();
        let sequences = crate::presentation::sequence_index(&manifest);
        for name in (1..=24)
            .map(|number| format!("die green {number}"))
            .chain((0..13).map(|number| format!("cornerhead {number}")))
        {
            anyhow::ensure!(
                sequences.contains_key(&name),
                "original animation sequence {name:?} is missing from MASTER.ALI"
            );
        }
        let effect_limit = manifest
            .resources
            .iter()
            .find(|table| table.id.to_ascii_uppercase().ends_with("VALUELST.RES"))
            .and_then(|table| table.commands.iter().find(|command| command.kind == "8"))
            .and_then(|command| command.fields.first())
            .and_then(|value| value.parse().ok())
            .unwrap_or(5);
        let mut this = Self {
            cache,
            manifest,
            textures: HashMap::new(),
            runtime_variants: HashMap::new(),
            sequences,
            sounds: HashMap::new(),
            music: None,
            font,
            warnings: Vec::new(),
            mute,
            volume,
            effect_limit,
        };
        let files: Vec<_> = this
            .font
            .iter()
            .map(|f| f.texture.clone())
            .chain(this.manifest.textures.iter().map(|t| t.path.clone()))
            .collect();
        for (i, file) in files.iter().enumerate() {
            this.ensure_texture(file).await?;
            if i % 8 == 0 {
                this.loading_progress((i + 1) as f32 / files.len() as f32 * 0.6)
                    .await;
            }
        }
        let animations = [
            "WALK.ANI",
            "STAND.ANI",
            "SHADOW.ANI",
            "BOMBANIM.ANI",
            "BOMBS.ANI",
            "TRIGBOMB.ANI",
            "TRIGANIM.ANI",
            "FLAME.ANI",
            "MFLAME.ANI",
            "POWERS.ANI",
            "CONVEYOR.ANI",
            "EXTRAS.ANI",
            "HURRY.ANI",
            "XPLODE1.ANI",
            "BWALK1.ANI",
            "BWALK2.ANI",
            "BWALK3.ANI",
            "BWALK4.ANI",
            "KICK.ANI",
            "DUDS.ANI",
            "MISC.ANI",
        ];
        for (i, name) in animations.iter().enumerate() {
            this.ensure_animation(name).await?;
            this.loading_progress(0.6 + 0.4 * (i + 1) as f32 / animations.len() as f32)
                .await;
        }
        Ok(this)
    }

    async fn loading_progress(&self, progress: f32) {
        set_camera(&Camera2D::from_display_rect(Rect::new(0., 0., 640., 480.)));
        clear_background(Color::new(0.015, 0.025, 0.06, 1.));
        self.center_text("ATOMIC BOMBERMAN", 192., 24., GOLD);
        self.center_text("Loading original artwork...", 238., 15., WHITE);
        draw_rectangle(120., 279., 400., 5., Color::new(0.12, 0.2, 0.32, 1.));
        draw_rectangle(120., 279., 400. * progress, 5., GOLD);
        set_default_camera();
        next_frame().await;
    }

    async fn ensure_texture(&mut self, file: &str) -> Result<()> {
        if self.textures.contains_key(file) {
            return Ok(());
        }
        let path = self.cache.join(file);
        let texture = load_texture(&path.to_string_lossy())
            .await
            .with_context(|| format!("load original artwork {}", path.display()))?;
        texture.set_filter(FilterMode::Nearest);
        self.textures.insert(file.into(), texture);
        Ok(())
    }

    pub fn animation(&self, name: &str) -> Option<&AnimationAsset> {
        self.manifest
            .animations
            .iter()
            .find(|a| a.id.eq_ignore_ascii_case(name) || basename(&a.id).eq_ignore_ascii_case(name))
    }

    pub async fn ensure_animation(&mut self, name: &str) -> Result<()> {
        let Some(animation) = self.animation(name).cloned() else {
            return Ok(());
        };
        let files: Vec<_> = std::iter::once(animation.texture.clone())
            .chain(animation.player_variants.iter().cloned())
            .collect();
        for file in files {
            self.ensure_texture(&file).await?;
        }
        if animation.player_variants.is_empty()
            && [
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
            .any(|name| basename(&animation.id).eq_ignore_ascii_case(name))
        {
            self.ensure_remaps(&animation).await?;
        }
        Ok(())
    }

    async fn ensure_remaps(&mut self, animation: &AnimationAsset) -> Result<()> {
        if self.runtime_variants.contains_key(&animation.id) {
            return Ok(());
        }
        let path = self.cache.join(&animation.indexed_path);
        let indexed = load_image(&path.to_string_lossy())
            .await
            .with_context(|| format!("load original palette indices {}", path.display()))?;
        let mut variants = Vec::new();
        for remap in &self.manifest.player_remaps {
            let mut rgba = Vec::with_capacity(indexed.bytes.len());
            for pixel in indexed.bytes.chunks_exact(4) {
                let index = pixel[0] as usize;
                rgba.extend(self.manifest.palette[remap[index] as usize]);
                rgba.push(if index == 0 { 0 } else { 255 });
            }
            let texture = Texture2D::from_rgba8(indexed.width, indexed.height, &rgba);
            texture.set_filter(FilterMode::Nearest);
            variants.push(texture);
        }
        self.runtime_variants.insert(animation.id.clone(), variants);
        Ok(())
    }

    fn sequence(&self, name: &str) -> Option<(&AnimationAsset, &AnimationSequence)> {
        let &(animation, sequence) = self.sequences.get(&name.to_ascii_lowercase())?;
        let animation = self.manifest.animations.get(animation)?;
        Some((animation, animation.sequences.get(sequence)?))
    }

    pub fn sequence_len(&self, name: &str) -> usize {
        self.sequence(name)
            .map_or(0, |(_, sequence)| sequence.frames.len())
    }

    /// Upload newly selected native action/death artwork without loading all
    /// ten color variants of every death animation at startup.
    pub async fn ensure_players(&mut self, players: &[Player]) -> Result<()> {
        self.ensure_players_with_colors(players, &bomberman_net::DEFAULT_COLORS)
            .await
    }

    pub async fn ensure_players_with_colors(
        &mut self,
        players: &[Player],
        colors: &[u8; 10],
    ) -> Result<()> {
        for player in players {
            let sprite = crate::presentation::player_sprite(player);
            let color = crate::presentation::player_palette(player, colors);
            self.ensure_sequence(&sprite.sequence, Some(color)).await?;
        }
        Ok(())
    }

    /// Prepare the same selected image and palette path used by battle actors.
    pub async fn ensure_sequence(&mut self, name: &str, variant: Option<usize>) -> Result<()> {
        let (animation, _) = self
            .sequence(name)
            .with_context(|| format!("original animation sequence {name:?} is missing"))?;
        let animation = animation.clone();
        self.ensure_texture(&animation.texture).await?;
        if let Some(variant) = variant {
            if let Some(file) = animation.player_variants.get(variant) {
                self.ensure_texture(file).await?;
            } else {
                self.ensure_remaps(&animation).await?;
            }
        }
        Ok(())
    }

    pub async fn ensure_arena(&mut self, index: usize) -> Result<()> {
        let arena = &self.manifest.arenas[index % self.manifest.arenas.len()];
        let brick_animation = arena.brick_animation.clone();
        let names = [arena.tiles.clone(), arena.brick_animation.clone()];
        for name in names {
            self.ensure_animation(&name).await?;
        }
        // Native shape9 uses the bomb's stored palette, just like floor flames.
        if let Some(animation) = self.animation(&brick_animation).cloned() {
            self.ensure_remaps(&animation).await?;
        }
        Ok(())
    }

    pub fn texture(&self, id: &str) -> Option<&Texture2D> {
        let file =
            self.manifest.textures.iter().find(|t| {
                t.id.eq_ignore_ascii_case(id) || basename(&t.id).eq_ignore_ascii_case(id)
            })?;
        self.textures.get(&file.path)
    }

    pub fn image(&self, id: &str, x: f32, y: f32, width: f32, height: f32) -> bool {
        if let Some(texture) = self.texture(id) {
            draw_texture_ex(
                texture,
                x,
                y,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(width, height)),
                    ..Default::default()
                },
            );
            true
        } else {
            false
        }
    }

    pub fn text(&self, value: &str, x: f32, y: f32, size: f32, color: Color) {
        let Some(font) = &self.font else {
            draw_text(value, x, y + size, size, color);
            return;
        };
        let Some(texture) = self.textures.get(&font.texture) else {
            return;
        };
        let scale = size / font.line_height.max(1) as f32;
        let mut cursor = x;
        let mut top = y;
        for ch in value.chars() {
            if ch == '\n' {
                cursor = x;
                top += size + 3.;
                continue;
            }
            let glyph = font
                .glyphs
                .iter()
                .find(|g| g.codepoint == ch as u32)
                .or_else(|| font.glyphs.iter().find(|g| g.codepoint == '?' as u32));
            if let Some(glyph) = glyph {
                draw_texture_ex(
                    texture,
                    cursor,
                    top,
                    color,
                    DrawTextureParams {
                        dest_size: Some(vec2(
                            glyph.width as f32 * scale,
                            glyph.height as f32 * scale,
                        )),
                        source: Some(Rect::new(
                            glyph.x as f32,
                            glyph.y as f32,
                            glyph.width as f32,
                            glyph.height as f32,
                        )),
                        ..Default::default()
                    },
                );
                cursor += glyph.advance as f32 * scale;
            } else {
                cursor += size * 0.5;
            }
        }
    }

    pub fn text_width(&self, value: &str, size: f32) -> f32 {
        self.font.as_ref().map_or_else(
            || measure_text(value, None, size as u16, 1.).width,
            |font| {
                value
                    .chars()
                    .map(|ch| {
                        font.glyphs
                            .iter()
                            .find(|g| g.codepoint == ch as u32)
                            .map_or(font.line_height / 2, |g| g.advance)
                            as f32
                    })
                    .sum::<f32>()
                    * size
                    / font.line_height.max(1) as f32
            },
        )
    }

    pub fn center_text(&self, value: &str, y: f32, size: f32, color: Color) {
        self.text(
            value,
            (640. - self.text_width(value, size)) * 0.5,
            y,
            size,
            color,
        );
    }

    fn draw_frame(
        &self,
        animation: &AnimationAsset,
        frame: &AnimationFrame,
        x: f32,
        y: f32,
        variant: Option<usize>,
    ) -> bool {
        let texture = variant
            .and_then(|v| {
                self.runtime_variants
                    .get(&animation.id)
                    .and_then(|textures| textures.get(v))
            })
            .or_else(|| {
                let file = variant
                    .and_then(|v| animation.player_variants.get(v))
                    .unwrap_or(&animation.texture);
                self.textures.get(file)
            });
        let Some(texture) = texture else {
            return false;
        };
        let (x, y) = crate::presentation::image_position(frame, x, y);
        draw_texture_ex(
            texture,
            x,
            y,
            WHITE,
            DrawTextureParams {
                source: Some(Rect::new(
                    frame.x as f32,
                    frame.y as f32,
                    frame.width as f32,
                    frame.height as f32,
                )),
                ..Default::default()
            },
        );
        true
    }

    /// Plays a sequence for interface artwork. Battle actors use native frame
    /// indices through `state` rather than CHFILE duration metadata.
    pub fn animate(
        &self,
        name: &str,
        wanted: &[&str],
        time_ms: u64,
        x: f32,
        y: f32,
        variant: Option<usize>,
    ) -> bool {
        let Some(animation) = self.animation(name) else {
            return false;
        };
        let sequence = wanted
            .iter()
            .find_map(|wanted| {
                animation
                    .sequences
                    .iter()
                    .find(|s| s.name.eq_ignore_ascii_case(wanted))
            })
            .or_else(|| {
                wanted.iter().find_map(|wanted| {
                    animation.sequences.iter().find(|s| {
                        s.name
                            .to_ascii_lowercase()
                            .contains(&wanted.to_ascii_lowercase())
                    })
                })
            })
            .or_else(|| {
                if wanted.is_empty() {
                    animation.sequences.first()
                } else {
                    None
                }
            });
        let Some(sequence) = sequence else {
            return wanted.is_empty()
                && animation
                    .frames
                    .first()
                    .is_some_and(|frame| self.draw_frame(animation, frame, x, y, variant));
        };
        if sequence.frames.is_empty() {
            return false;
        }
        let total: u64 = sequence
            .frames
            .iter()
            .enumerate()
            .map(|(i, _)| sequence.durations_ms.get(i).copied().unwrap_or(30).max(1) as u64)
            .sum();
        let mut local = time_ms % total.max(1);
        let mut selected = 0;
        for (i, _) in sequence.frames.iter().enumerate() {
            let duration = sequence.durations_ms.get(i).copied().unwrap_or(30).max(1) as u64;
            selected = i;
            if local < duration {
                break;
            }
            local -= duration;
        }
        self.draw_state(animation, sequence, selected, x, y, variant)
    }

    fn draw_state(
        &self,
        animation: &AnimationAsset,
        sequence: &AnimationSequence,
        selected: usize,
        x: f32,
        y: f32,
        variant: Option<usize>,
    ) -> bool {
        // 0x41daa7 dereferences the first FRAM record's image ID only. Its
        // following words are separately queried metadata, not placement.
        let Some(frame) = crate::presentation::native_image(sequence, selected)
            .and_then(|index| animation.frames.get(index as usize))
        else {
            return false;
        };
        self.draw_frame(animation, frame, x, y, variant)
    }

    /// The executable indexes sequence states directly and takes the remainder
    /// by the sequence length (0x41daa7); CHFILE duration metadata is not used.
    pub fn state(&self, name: &str, frame: usize, x: f32, y: f32, variant: Option<usize>) -> bool {
        let Some((animation, sequence)) = self.sequence(name) else {
            return false;
        };
        if sequence.frames.is_empty() {
            return false;
        }
        self.draw_state(
            animation,
            sequence,
            frame % sequence.frames.len(),
            x,
            y,
            variant,
        )
    }

    /// Original 0x41db41 exposes the two metadata words of the first FRAM.
    /// Flame placement explicitly uses the second word for horizontal alignment.
    pub fn state_metadata(&self, name: &str, frame: usize) -> [i32; 2] {
        self.sequence(name).map_or([0, 0], |(_, sequence)| {
            crate::presentation::native_metadata(sequence, frame)
        })
    }

    async fn sound(&mut self, stem: &str) -> Option<String> {
        if self.mute {
            return None;
        }
        let stem = basename(stem).split('.').next().unwrap_or(stem);
        let sound = self.manifest.sounds.iter().find(|s| {
            basename(&s.id)
                .split('.')
                .next()
                .is_some_and(|n| n.eq_ignore_ascii_case(stem))
        })?;
        let key = sound.id.clone();
        if !self.sounds.contains_key(&key) {
            let path = self.cache.join(&sound.path);
            match load_sound(&path.to_string_lossy()).await {
                Ok(sound) => {
                    self.sounds.insert(key.clone(), sound);
                }
                Err(error) => {
                    self.warnings
                        .push(format!("Audio {} unavailable: {error}", path.display()));
                    return None;
                }
            }
        }
        Some(key)
    }

    pub async fn music(&mut self, stem: &str) {
        if self.music.as_ref().is_some_and(|id| {
            basename(id)
                .split('.')
                .next()
                .is_some_and(|s| s.eq_ignore_ascii_case(stem))
        }) {
            return;
        }
        if let Some(old) = self.music.take() {
            if let Some(sound) = self.sounds.get(&old) {
                stop_sound(sound);
            }
        }
        if let Some(key) = self.sound(stem).await {
            play_sound(
                &self.sounds[&key],
                PlaySoundParams {
                    looped: true,
                    volume: self.volume * 0.55,
                },
            );
            self.music = Some(key);
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        if let Some(sound) = self.music.as_ref().and_then(|id| self.sounds.get(id)) {
            set_sound_volume(sound, volume * 0.55);
        }
    }

    pub async fn events(&mut self, events: &[GameEvent]) {
        // The simulation has already applied original contiguous-group lookup,
        // play-count balancing, RNG consumption and event order. Missing disc
        // placeholders stay silent, as they do when the original file fails.
        for event in events {
            let GameEvent::Sound { number } = event else {
                continue;
            };
            // 0x427859 checks its live sample count before looking up or
            // loading a clip. Count pending mixer commands too, so a burst
            // in one update preserves the original inclusive sixth voice.
            if !crate::audio_policy::permits_effect(quad_snd::active_one_shots(), self.effect_limit)
            {
                continue;
            }
            let Some(stem) = self
                .manifest
                .sound_mappings
                .iter()
                .find(|mapping| mapping.number == *number)
                .map(|mapping| mapping.sound.clone())
            else {
                continue;
            };
            if let Some(key) = self.sound(&stem).await {
                play_sound(
                    &self.sounds[&key],
                    PlaySoundParams {
                        looped: false,
                        volume: self.volume,
                    },
                );
            }
        }
    }
}

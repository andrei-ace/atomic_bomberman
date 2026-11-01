//! Original .BM document viewer. Imported text remains the source of layout;
//! inline pictures share the text baseline rather than inserting extra lines.
use crate::{controls::Controllers, media::Media};
use bomberman_assets::{FontAsset, Manifest};
use macroquad::prelude::*;
use std::{cell::RefCell, collections::HashMap};

const FONT: &str = "FONT1.FON";
const LINE_HEIGHT: usize = 16;
// BM95 0x41302d: 0x22px window inset, 0x214px text width and 0x158px page.
const BODY_X: f32 = 54.;
const BODY_Y: f32 = 54.;
const BODY_WIDTH: usize = 532;
const BODY_HEIGHT: usize = 344;

#[derive(Default)]
pub(super) struct DocumentView {
    pub(super) name: String,
    /// Scroll position in original screen pixels.
    pub(super) scroll: usize,
    layout: Layout,
    mouse: Vec2,
    pictures: RefCell<HashMap<String, Texture2D>>,
}

#[derive(Debug, Default)]
struct Layout {
    items: Vec<Item>,
    height: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum Item {
    Text {
        value: String,
        x: usize,
        y: i32,
    },
    Picture {
        id: String,
        x: usize,
        y: i32,
        width: usize,
        height: usize,
    },
}

struct Metrics {
    advances: [usize; 256],
    fallback: usize,
}

impl Metrics {
    fn new(font: Option<&FontAsset>) -> Self {
        let mut result = Self {
            advances: [8; 256],
            fallback: 8,
        };
        if let Some(font) = font {
            for glyph in &font.glyphs {
                if let Some(advance) = result.advances.get_mut(glyph.codepoint as usize) {
                    *advance = glyph.advance as usize;
                }
            }
            result.fallback = result.advances[b'?' as usize];
        }
        result
    }

    fn advance(&self, ch: char) -> usize {
        self.advances
            .get(ch as usize)
            .copied()
            .unwrap_or(self.fallback)
    }

    fn width(&self, text: &str) -> usize {
        text.chars().map(|ch| self.advance(ch)).sum()
    }
}

/// All document resources supplied by this edition, in the original picker order.
pub(super) fn help_files(manifest: &Manifest) -> Vec<String> {
    let mut files: Vec<_> = manifest
        .resources
        .iter()
        .filter(|table| table.id.to_ascii_uppercase().ends_with(".BM"))
        .map(|table| basename(&table.id).to_owned())
        .collect();
    files.sort_by_cached_key(|name| name.to_ascii_uppercase());
    files.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    files
}

fn basename(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

impl DocumentView {
    #[cfg(debug_assertions)]
    pub(super) fn title(&self) -> Option<&str> {
        self.layout.items.iter().find_map(|item| {
            if let Item::Text { value, .. } = item {
                Some(value.as_str())
            } else {
                None
            }
        })
    }
    pub(super) fn open(&mut self, manifest: &Manifest, name: &str) -> bool {
        let requested = if name.to_ascii_uppercase().ends_with(".BM") {
            basename(name).to_owned()
        } else {
            format!("{name}.BM")
        };
        let Some(table) = manifest
            .resources
            .iter()
            .find(|table| basename(&table.id).eq_ignore_ascii_case(&requested))
        else {
            return false;
        };
        let metrics = Metrics::new(
            manifest
                .fonts
                .iter()
                .find(|font| font.id.eq_ignore_ascii_case(FONT)),
        );
        let pictures = manifest
            .textures
            .iter()
            .map(|texture| {
                (
                    basename(&texture.id).to_ascii_uppercase(),
                    PictureAsset {
                        id: texture.id.clone(),
                        width: texture.width as usize,
                        height: texture.height as usize,
                    },
                )
            })
            .collect();
        let text = super::help::current(&requested).unwrap_or(&table.text);
        self.layout = layout_document(text, &metrics, &pictures, BODY_WIDTH);
        self.name = basename(&table.id).to_owned();
        self.scroll = 0;
        true
    }

    fn max_scroll(&self) -> usize {
        self.layout.height.saturating_sub(BODY_HEIGHT)
    }

    fn scroll_by(&mut self, amount: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(amount)
            .min(self.max_scroll());
    }

    /// Returns true when the caller should restore the previous game screen.
    pub(super) fn update(&mut self, controllers: &Controllers, mouse: Vec2, clicked: bool) -> bool {
        self.mouse = mouse;
        if is_key_pressed(KeyCode::Escape)
            || is_key_pressed(KeyCode::Enter)
            || controllers.back
            || controllers.confirm
        {
            return true;
        }
        if is_key_pressed(KeyCode::Up) || controllers.up {
            self.scroll_by(-(LINE_HEIGHT as isize));
        }
        if is_key_pressed(KeyCode::Down) || controllers.down {
            self.scroll_by(LINE_HEIGHT as isize);
        }
        let page = (BODY_HEIGHT / LINE_HEIGHT * LINE_HEIGHT) as isize;
        if is_key_pressed(KeyCode::PageUp) || is_key_pressed(KeyCode::Left) || controllers.left {
            self.scroll_by(-page);
        }
        if is_key_pressed(KeyCode::PageDown) || is_key_pressed(KeyCode::Right) || controllers.right
        {
            self.scroll_by(page);
        }
        if is_key_pressed(KeyCode::Home) {
            self.scroll = 0;
        }
        if is_key_pressed(KeyCode::End) {
            self.scroll = self.max_scroll();
        }
        let wheel = mouse_wheel().1;
        if wheel.is_finite() && wheel != 0. {
            self.scroll_by((-wheel * (LINE_HEIGHT * 3) as f32).round() as isize);
        }
        if clicked {
            for (index, button) in buttons().iter().enumerate() {
                if button.contains(mouse) {
                    match index {
                        0 => self.scroll_by(-(LINE_HEIGHT as isize)),
                        1 => self.scroll_by(LINE_HEIGHT as isize),
                        2 => self.scroll_by(-page),
                        3 => self.scroll_by(page),
                        _ => return true,
                    }
                }
            }
        }
        self.scroll = self.scroll.min(self.max_scroll());
        false
    }

    pub(super) fn draw(&self, media: &Media) {
        blue_panel(20., 20., 600., 440.);
        let top = self.scroll as i32;
        let bottom = top + BODY_HEIGHT as i32;
        for item in &self.layout.items {
            match item {
                Item::Text { value, x, y } => {
                    if *y >= top && y + LINE_HEIGHT as i32 <= bottom {
                        media.font_text(
                            FONT,
                            value,
                            BODY_X + *x as f32,
                            BODY_Y + (*y - top) as f32,
                            LINE_HEIGHT as f32,
                            WHITE,
                        );
                    }
                }
                Item::Picture {
                    id,
                    x,
                    y,
                    width,
                    height,
                } => {
                    if *y >= bottom || y + *height as i32 <= top {
                        continue;
                    }
                    self.prepare_picture(media, id);
                    if let Some(texture) = self.pictures.borrow().get(id) {
                        let visible_top = (*y).max(top);
                        let visible_bottom = (y + *height as i32).min(bottom);
                        let visible_width = (*width).min(BODY_WIDTH.saturating_sub(*x));
                        if visible_width == 0 || visible_top >= visible_bottom {
                            continue;
                        }
                        draw_texture_ex(
                            texture,
                            BODY_X + *x as f32,
                            BODY_Y + (visible_top - top) as f32,
                            WHITE,
                            DrawTextureParams {
                                source: Some(Rect::new(
                                    0.,
                                    (visible_top - *y) as f32,
                                    visible_width as f32,
                                    (visible_bottom - visible_top) as f32,
                                )),
                                ..Default::default()
                            },
                        );
                    }
                }
            }
        }
        for (rect, label) in buttons().iter().zip(["^", "v", "Page -", "Page +", "Done"]) {
            gray_button(*rect, rect.contains(self.mouse));
            media.font_text(
                "FONT0.FON",
                label,
                rect.x + 4.,
                rect.y + 1.,
                17.,
                Color::new(0.92, 0.92, 0.92, 1.),
            );
        }
    }

    fn prepare_picture(&self, media: &Media, id: &str) {
        if self.pictures.borrow().contains_key(id) {
            return;
        }
        if let Some(texture) = media.texture(id) {
            let mut image = texture.get_texture_data();
            key_black_pixels(&mut image.bytes);
            let keyed = Texture2D::from_image(&image);
            keyed.set_filter(FilterMode::Nearest);
            self.pictures.borrow_mut().insert(id.to_owned(), keyed);
        }
    }
}

fn key_black_pixels(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        if pixel[..3] == [0, 0, 0] {
            pixel[3] = 0;
        }
    }
}

fn buttons() -> [Rect; 5] {
    [
        Rect::new(50., 410., 24., 20.),
        Rect::new(80., 410., 24., 20.),
        Rect::new(140., 410., 70., 20.),
        Rect::new(210., 410., 70., 20.),
        Rect::new(535., 410., 50., 20.),
    ]
}

/// The blue beveled frame used by the original document and settings dialogs.
pub(super) fn blue_panel(x: f32, y: f32, width: f32, height: f32) {
    draw_rectangle(x, y, width, height, Color::new(0.04, 0.07, 0.17, 1.));
    draw_rectangle(
        x + 2.,
        y + 2.,
        width - 4.,
        height - 4.,
        Color::new(0.25, 0.34, 0.48, 1.),
    );
    draw_rectangle(
        x + 5.,
        y + 5.,
        width - 10.,
        height - 10.,
        Color::new(0.10, 0.16, 0.30, 1.),
    );
    draw_rectangle(
        x + 9.,
        y + 9.,
        width - 18.,
        height - 18.,
        Color::new(0.01, 0.015, 0.20, 1.),
    );
    draw_line(
        x + 6.,
        y + 6.,
        x + width - 6.,
        y + 6.,
        2.,
        Color::new(0.40, 0.48, 0.61, 1.),
    );
    draw_line(
        x + 6.,
        y + 6.,
        x + 6.,
        y + height - 6.,
        2.,
        Color::new(0.40, 0.48, 0.61, 1.),
    );
}

fn gray_button(rect: Rect, hover: bool) {
    draw_rectangle(rect.x, rect.y, rect.w, rect.h, BLACK);
    draw_rectangle(
        rect.x,
        rect.y,
        rect.w - 1.,
        rect.h - 1.,
        if hover {
            Color::new(0.55, 0.57, 0.58, 1.)
        } else {
            Color::new(0.43, 0.45, 0.46, 1.)
        },
    );
    draw_line(
        rect.x,
        rect.y,
        rect.x + rect.w - 1.,
        rect.y,
        1.,
        Color::new(0.8, 0.8, 0.8, 1.),
    );
    draw_line(
        rect.x,
        rect.y,
        rect.x,
        rect.y + rect.h - 1.,
        1.,
        Color::new(0.8, 0.8, 0.8, 1.),
    );
}

struct PictureAsset {
    id: String,
    width: usize,
    height: usize,
}

fn expand_native_tabs(source: &str) -> String {
    let mut expanded = String::with_capacity(source.len());
    let mut column = 0usize;
    for ch in source.chars() {
        if ch == '\n' {
            expanded.push(ch);
            column = 0;
            continue;
        }
        if ch == '\t' {
            // BM95 0x41302d emits spaces to a four-character stop, then
            // advances its logical column once more for the source tab.
            // Adjacent tabs consequently emit four, three, three... spaces.
            loop {
                expanded.push(' ');
                column += 1;
                if column & 3 == 0 {
                    break;
                }
            }
        } else {
            expanded.push(ch);
        }
        column += 1;
    }
    expanded
}

fn layout_document(
    source: &str,
    metrics: &Metrics,
    pictures: &HashMap<String, PictureAsset>,
    width: usize,
) -> Layout {
    let source = source.split('\u{1a}').next().unwrap_or_default();
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let normalized = expand_native_tabs(&normalized);
    let mut remaining = normalized.as_str();
    let mut result = Layout::default();
    let (mut x, mut y) = (0usize, 0i32);
    while let Some(ch) = remaining.chars().next() {
        remaining = &remaining[ch.len_utf8()..];
        match ch {
            '\n' => {
                x = 0;
                y += LINE_HEIGHT as i32;
            }
            '<' => {
                if let Some(end) = remaining.find('>') {
                    let tag = &remaining[..end];
                    if let Some(name) = tag
                        .get(..3)
                        .filter(|prefix| prefix.eq_ignore_ascii_case("IMG"))
                        .map(|_| &tag[3..])
                    {
                        if let Some(picture) =
                            pictures.get(&format!("{}.PCX", name.to_ascii_uppercase()))
                        {
                            if x > 0 && x + picture.width > width {
                                x = 0;
                                y += LINE_HEIGHT as i32;
                            }
                            let image_y = y + LINE_HEIGHT as i32 / 2 - picture.height as i32 / 2;
                            result.items.push(Item::Picture {
                                id: picture.id.clone(),
                                x,
                                y: image_y,
                                width: picture.width,
                                height: picture.height,
                            });
                            result.height = result
                                .height
                                .max((image_y + picture.height as i32).max(0) as usize);
                            x += picture.width;
                            remaining = &remaining[end + 1..];
                            continue;
                        }
                    }
                }
                append_text(&mut result, "<", &mut x, &mut y, metrics, width);
            }
            ch if ch.is_whitespace() => {
                let mut value = ch.to_string();
                let len = remaining
                    .char_indices()
                    .find(|(_, ch)| !ch.is_whitespace() || matches!(ch, '\t' | '\n'))
                    .map_or(remaining.len(), |(index, _)| index);
                value.push_str(&remaining[..len]);
                remaining = &remaining[len..];
                append_text(&mut result, &value, &mut x, &mut y, metrics, width);
            }
            ch => {
                let mut value = ch.to_string();
                let len = remaining
                    .char_indices()
                    .find(|(_, ch)| ch.is_whitespace() || *ch == '<')
                    .map_or(remaining.len(), |(index, _)| index);
                value.push_str(&remaining[..len]);
                remaining = &remaining[len..];
                if x > 0 && x + metrics.width(&value) > width {
                    x = 0;
                    y += LINE_HEIGHT as i32;
                }
                append_text(&mut result, &value, &mut x, &mut y, metrics, width);
            }
        }
    }
    result.height = result.height.max(y.max(0) as usize + LINE_HEIGHT);
    result
}

fn append_text(
    result: &mut Layout,
    text: &str,
    x: &mut usize,
    y: &mut i32,
    metrics: &Metrics,
    width: usize,
) {
    for ch in text.chars() {
        let advance = metrics.advance(ch);
        if *x > 0 && *x + advance > width {
            *x = 0;
            *y += LINE_HEIGHT as i32;
        }
        match result.items.last_mut() {
            Some(Item::Text {
                value,
                x: start,
                y: row,
            }) if *row == *y && *start + metrics.width(value) == *x => {
                value.push(ch);
            }
            _ => result.items.push(Item::Text {
                value: ch.to_string(),
                x: *x,
                y: *y,
            }),
        }
        *x += advance;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_metrics() -> Metrics {
        Metrics::new(None)
    }

    #[test]
    fn source_lines_tabs_and_dos_eof_survive_layout() {
        let result = layout_document(
            "Title\r\n\tA\tB\r\n\r\nLast\u{1a}not part of document",
            &fixed_metrics(),
            &HashMap::new(),
            500,
        );
        assert_eq!(
            result.items,
            [
                Item::Text {
                    value: "Title".into(),
                    x: 0,
                    y: 0
                },
                Item::Text {
                    value: "    A  B".into(),
                    x: 0,
                    y: 16
                },
                Item::Text {
                    value: "Last".into(),
                    x: 0,
                    y: 48
                },
            ]
        );
        assert_eq!(result.height, 64);
    }

    #[test]
    fn adjacent_tabs_follow_native_character_columns_including_the_source_tab() {
        assert_eq!(expand_native_tabs("\t\t\t"), "          ");
        assert_eq!(expand_native_tabs("A\tB\n\tC"), "A   B\n    C");
        // The two author portraits have distinct tab/space prefixes but the
        // original viewer places both at the same horizontal pixel position.
        let jeremy = expand_native_tabs("\t\t            ");
        let kurt = expand_native_tabs("\t\t\t         ");
        assert_eq!(jeremy, " ".repeat(19));
        assert_eq!(jeremy, kurt);
        assert_eq!(expand_native_tabs("\t\t\t\t\t    "), " ".repeat(20));
    }

    #[test]
    fn picture_tags_are_case_insensitive_and_centered_on_the_text_line() {
        let pictures = HashMap::from([(
            "POWBOMB.PCX".into(),
            PictureAsset {
                id: "DATA/RES/POWBOMB.PCX".into(),
                width: 40,
                height: 36,
            },
        )]);
        let result = layout_document(
            "x <iMgpowbomb> Next\nSecond",
            &fixed_metrics(),
            &pictures,
            300,
        );
        assert!(result.items.contains(&Item::Picture {
            id: "DATA/RES/POWBOMB.PCX".into(),
            x: 16,
            y: -10,
            width: 40,
            height: 36,
        }));
        assert!(result.items.contains(&Item::Text {
            value: " Next".into(),
            x: 56,
            y: 0
        }));
        assert!(result.items.contains(&Item::Text {
            value: "Second".into(),
            x: 0,
            y: 16
        }));
        assert_eq!(result.height, 32);
    }

    #[test]
    fn wrapping_keeps_words_together_and_splits_a_long_word_at_the_edge() {
        let result = layout_document("one two\nabcdefg", &fixed_metrics(), &HashMap::new(), 40);
        assert_eq!(
            result.items,
            [
                Item::Text {
                    value: "one ".into(),
                    x: 0,
                    y: 0
                },
                Item::Text {
                    value: "two".into(),
                    x: 0,
                    y: 16
                },
                Item::Text {
                    value: "abcde".into(),
                    x: 0,
                    y: 32
                },
                Item::Text {
                    value: "fg".into(),
                    x: 0,
                    y: 48
                },
            ]
        );
    }

    #[test]
    fn missing_picture_tags_remain_visible_text() {
        let result = layout_document("<IMGmissing>", &fixed_metrics(), &HashMap::new(), 500);
        assert_eq!(
            result.items,
            [Item::Text {
                value: "<IMGmissing>".into(),
                x: 0,
                y: 0
            }]
        );
    }

    #[test]
    fn scrolling_clamps_both_short_and_long_documents() {
        let mut view = DocumentView::default();
        view.scroll_by(1000);
        assert_eq!(view.scroll, 0);
        view.layout.height = 1000;
        view.scroll_by(isize::MAX);
        assert_eq!(view.scroll, 656);
        view.scroll_by(isize::MIN);
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn only_exact_black_is_transparent_in_inline_pictures() {
        let mut pixels = [0, 0, 0, 255, 1, 0, 0, 255, 255, 255, 0, 255];
        key_black_pixels(&mut pixels);
        assert_eq!(pixels, [0, 0, 0, 0, 1, 0, 0, 255, 255, 255, 0, 255]);
    }
}

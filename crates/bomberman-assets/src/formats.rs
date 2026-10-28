//! Parsers for the formats on the supplied English disc. All lengths and offsets
//! are checked before use; no original executables or decoders are run.
use crate::manifest::*;
use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeSet;

const MAX_PIXELS: usize = 16 * 1024 * 1024;

pub(crate) fn u16le(data: &[u8], off: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        data.get(off..off + 2)
            .context("truncated 16-bit field")?
            .try_into()?,
    ))
}
fn i16le(data: &[u8], off: usize) -> Result<i16> {
    Ok(u16le(data, off)? as i16)
}
pub(crate) fn u32le(data: &[u8], off: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        data.get(off..off + 4)
            .context("truncated 32-bit field")?
            .try_into()?,
    ))
}
fn area(w: usize, h: usize) -> Result<usize> {
    ensure!(
        w > 0 && h > 0 && w <= 8192 && h <= 8192,
        "invalid image dimensions {w}x{h}"
    );
    let n = w.checked_mul(h).context("image dimensions overflow")?;
    ensure!(n <= MAX_PIXELS, "image exceeds pixel limit");
    Ok(n)
}

pub(crate) struct Palette {
    pub rgb: Vec<[u8; 3]>,
    pub lookup: Vec<u8>,
}

impl Palette {
    pub fn parse(data: &[u8]) -> Result<Self> {
        ensure!(
            data.len() == 768 + 32768,
            "COLOR.PAL must contain a 768-byte VGA palette and 32768-byte RGB555 lookup"
        );
        ensure!(
            data[3..768].iter().all(|&b| b <= 63),
            "COLOR.PAL contains invalid VGA components"
        );
        let rgb = data[..768]
            .chunks_exact(3)
            .map(|c| {
                [
                    c[0].saturating_mul(4),
                    c[1].saturating_mul(4),
                    c[2].saturating_mul(4),
                ]
            })
            .collect();
        Ok(Self {
            rgb,
            lookup: data[768..].to_vec(),
        })
    }
    pub fn rgba(&self, indices: &[u8], remap: Option<&[u8]>, transparent: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(indices.len() * 4);
        for &original in indices {
            let index = remap.map_or(original, |r| r[original as usize]);
            out.extend(self.rgb[index as usize]);
            out.push(if transparent && original == 0 { 0 } else { 255 });
        }
        out
    }
}

pub(crate) struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub(crate) fn pcx(data: &[u8]) -> Result<Raster> {
    ensure!(data.len() >= 128, "truncated PCX header");
    ensure!(
        data[0] == 10 && data[2] <= 1 && data[3] == 8,
        "unsupported PCX manufacturer, encoding, or bit depth"
    );
    let x0 = u16le(data, 4)?;
    let y0 = u16le(data, 6)?;
    let x1 = u16le(data, 8)?;
    let y1 = u16le(data, 10)?;
    ensure!(x1 >= x0 && y1 >= y0, "PCX coordinates are inverted");
    let w = (x1 - x0) as usize + 1;
    let h = (y1 - y0) as usize + 1;
    let count = area(w, h)?;
    let planes = data[65] as usize;
    ensure!(
        planes == 1 || planes == 3,
        "unsupported PCX planes {planes}"
    );
    let stride = u16le(data, 66)? as usize;
    ensure!(
        stride >= w && stride <= 16384,
        "invalid PCX scanline stride"
    );
    let expected = stride
        .checked_mul(planes)
        .and_then(|n| n.checked_mul(h))
        .context("PCX stride overflow")?;
    ensure!(expected <= MAX_PIXELS * 4, "PCX decoded data exceeds limit");
    let end = if planes == 1 {
        ensure!(
            data.len() >= 128 + 769 && data[data.len() - 769] == 12,
            "PCX 256-color palette missing"
        );
        data.len() - 769
    } else {
        data.len()
    };
    let mut scanlines = Vec::with_capacity(expected);
    let mut pos = 128;
    while scanlines.len() < expected {
        let &value = data
            .get(pos)
            .filter(|_| pos < end)
            .context("truncated PCX image data")?;
        pos += 1;
        let (len, value) = if data[2] == 1 && value & 0xc0 == 0xc0 {
            let n = (value & 63) as usize;
            ensure!(n != 0, "zero-length PCX RLE run");
            let &v = data
                .get(pos)
                .filter(|_| pos < end)
                .context("truncated PCX RLE value")?;
            pos += 1;
            (n, v)
        } else {
            (1, value)
        };
        ensure!(
            scanlines.len() + len <= expected,
            "PCX run exceeds image size"
        );
        scanlines.resize(scanlines.len() + len, value);
    }
    ensure!(pos == end, "unexpected trailing PCX image bytes");
    let mut rgba = Vec::with_capacity(count * 4);
    let palette = if planes == 1 {
        &data[data.len() - 768..]
    } else {
        &[]
    };
    for y in 0..h {
        for x in 0..w {
            if planes == 1 {
                let index = scanlines[y * stride + x] as usize;
                rgba.extend_from_slice(&palette[3 * index..3 * index + 3]);
            } else {
                let row = y * stride * 3;
                rgba.extend([
                    scanlines[row + x],
                    scanlines[row + stride + x],
                    scanlines[row + 2 * stride + x],
                ]);
            }
            rgba.push(255);
        }
    }
    Ok(Raster {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}

struct Chunk<'a> {
    tag: &'a [u8],
    data: &'a [u8],
}
fn chunks(mut bytes: &[u8]) -> Result<Vec<Chunk<'_>>> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        ensure!(bytes.len() >= 10, "truncated CHFILE chunk header");
        let size = u32le(bytes, 4)? as usize;
        let end = size
            .checked_add(10)
            .context("CHFILE chunk length overflow")?;
        ensure!(end <= bytes.len(), "CHFILE chunk exceeds parent boundary");
        out.push(Chunk {
            tag: &bytes[..4],
            data: &bytes[10..end],
        });
        bytes = &bytes[end..];
    }
    Ok(out)
}

pub(crate) struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub origin_x: i32,
    pub origin_y: i32,
    pub indices: Vec<u8>,
    pub source_name: String,
}
pub(crate) struct Animation {
    pub frames: Vec<DecodedFrame>,
    pub sequences: Vec<AnimationSequence>,
}

/// RLE tokens use 1..127 literal/repeated pixels. 0xff terminates a stream.
/// Observed in BM95.EXE 0x41c0ba (16-bit), plus the unused CLASSICS.ANI 8-bit variant.
fn rle(data: &[u8], bytes_per_pixel: usize, decoded_len: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(decoded_len);
    let mut pos = 0;
    let mut terminated = false;
    while pos < data.len() {
        let ctrl = data[pos];
        pos += 1;
        if ctrl == 255 {
            terminated = true;
            break;
        }
        let n = ((ctrl & 127) as usize + 1) * bytes_per_pixel;
        ensure!(
            out.len() + n <= decoded_len,
            "CHFILE RLE output exceeds declared dimensions"
        );
        if ctrl & 128 != 0 {
            let end = pos
                .checked_add(bytes_per_pixel)
                .context("CHFILE RLE offset overflow")?;
            let px = data
                .get(pos..end)
                .context("truncated CHFILE RLE repeated pixel")?;
            for _ in 0..n / bytes_per_pixel {
                out.extend_from_slice(px);
            }
            pos = end;
        } else {
            let end = pos.checked_add(n).context("CHFILE RLE offset overflow")?;
            out.extend_from_slice(data.get(pos..end).context("truncated CHFILE RLE literal")?);
            pos = end;
        }
    }
    ensure!(
        out.len() == decoded_len,
        "CHFILE RLE decoded {} bytes, expected {decoded_len}",
        out.len()
    );
    ensure!(
        terminated && pos == data.len(),
        "CHFILE RLE end marker missing or trailing bytes present"
    );
    Ok(out)
}

fn cimg(data: &[u8], palette: &Palette) -> Result<DecodedFrame> {
    ensure!(data.len() >= 36, "truncated CIMG header");
    let format = u16le(data, 0)? & 7;
    ensure!(
        format == 4 || format == 3,
        "unsupported CIMG pixel format {format}"
    );
    let flags = u16le(data, 2)?;
    let offset = u32le(data, 4)? as usize;
    ensure!(
        offset >= 24 && offset <= data.len() - 12,
        "CIMG image offset outside chunk"
    );
    let w = u16le(data, 12)? as usize;
    let h = u16le(data, 14)? as usize;
    let pixels = area(w, h)?;
    let key = u32le(data, 20)?;
    let compression = data[offset];
    let header_size = u16le(data, offset + 2)? as usize;
    ensure!(header_size >= 12, "invalid CIMG compression header size");
    let packed = u32le(data, offset + 4)? as usize;
    let unpacked = u32le(data, offset + 8)? as usize;
    ensure!(
        packed >= header_size && offset.checked_add(packed) == Some(data.len()),
        "CIMG compressed length exceeds chunk"
    );
    let pixel_bytes = if format == 4 { 2 } else { 1 };
    ensure!(
        unpacked == pixels * pixel_bytes,
        "CIMG dimensions disagree with decoded byte count"
    );
    let image = &data[offset + header_size..offset + packed];
    let raw = match compression {
        0 => {
            ensure!(
                image.len() == unpacked,
                "raw CIMG image has wrong byte count"
            );
            image.to_vec()
        }
        0x11 if pixel_bytes == 2 => rle(image, 2, unpacked)?,
        0x10 if pixel_bytes == 1 => rle(image, 1, unpacked)?,
        _ => bail!("unsupported CIMG compression 0x{compression:x}"),
    };
    let mut indices = Vec::with_capacity(pixels);
    if pixel_bytes == 2 {
        for px in raw.chunks_exact(2) {
            let px = u16::from_le_bytes(px.try_into()?) as usize;
            ensure!(
                px < palette.lookup.len(),
                "CIMG RGB555 pixel outside palette lookup"
            );
            indices.push(if flags & 4 != 0 && px as u32 == key {
                0
            } else {
                palette.lookup[px]
            });
        }
    } else {
        // CLASSICS.ANI is not listed by MASTER.ALI. Its private RGBA palette is
        // still retained as accurately as the game's global indexed cache permits.
        ensure!(offset >= 1056, "indexed CIMG palette missing");
        for px in raw {
            let pos = 32 + 4 * px as usize;
            let [r, g, b] = [data[pos], data[pos + 1], data[pos + 2]];
            let color = ((r as usize >> 3) << 10) | ((g as usize >> 3) << 5) | (b as usize >> 3);
            indices.push(if flags & 4 != 0 && px as u32 == key {
                0
            } else {
                palette.lookup[color]
            });
        }
    }
    Ok(DecodedFrame {
        width: w as u32,
        height: h as u32,
        origin_x: i16le(data, 16)? as i32,
        origin_y: i16le(data, 18)? as i32,
        indices,
        source_name: String::new(),
    })
}

pub(crate) fn animation(data: &[u8], palette: &Palette) -> Result<Animation> {
    ensure!(
        data.len() >= 16 && &data[..10] == b"CHFILEANI ",
        "invalid CHFILE ANI signature"
    );
    ensure!(
        u32le(data, 10)? as usize == data.len() - 16,
        "CHFILE root length disagrees with file size"
    );
    let records = chunks(&data[16..])?;
    let mut frames = Vec::new();
    for record in records.iter().filter(|r| r.tag == b"FRAM") {
        let nested = chunks(record.data)?;
        let images: Vec<_> = nested.iter().filter(|r| r.tag == b"CIMG").collect();
        ensure!(images.len() == 1, "animation frame must contain one CIMG");
        let mut frame = cimg(images[0].data, palette)?;
        if let Some(name) = nested.iter().find(|r| r.tag == b"FNAM") {
            frame.source_name =
                String::from_utf8_lossy(name.data.split(|&b| b == 0).next().unwrap_or_default())
                    .into_owned();
        }
        frames.push(frame);
        ensure!(frames.len() <= 4096, "too many frames in animation");
    }
    ensure!(!frames.is_empty(), "animation has no frames");
    let mut sequences = Vec::new();
    for record in records.iter().filter(|r| r.tag == b"SEQ ") {
        let nested = chunks(record.data)?;
        let head = nested
            .iter()
            .find(|r| r.tag == b"HEAD")
            .context("animation sequence HEAD missing")?;
        ensure!(head.data.len() >= 84, "truncated animation sequence HEAD");
        let name = String::from_utf8_lossy(
            head.data[..42]
                .split(|&b| b == 0)
                .next()
                .unwrap_or_default(),
        )
        .into_owned();
        let mut seq = AnimationSequence {
            name,
            frames: Vec::new(),
            durations_ms: Vec::new(),
            offsets: Vec::new(),
            states: Vec::new(),
        };
        let mut previous_duration = 30;
        for state in nested.iter().filter(|r| r.tag == b"STAT") {
            let parts = chunks(state.data)?;
            let header = parts
                .iter()
                .find(|r| r.tag == b"HEAD")
                .context("animation STAT HEAD missing")?;
            ensure!(header.data.len() >= 6, "truncated STAT HEAD");
            let duration_raw = i16le(header.data, 0)?;
            // -1 is the editor's inherited duration marker.
            ensure!(
                duration_raw == -1 || duration_raw > 0,
                "invalid animation state duration"
            );
            let duration_ms = if duration_raw == -1 {
                previous_duration
            } else {
                duration_raw as u32
            };
            previous_duration = duration_ms;
            let placement = parts
                .iter()
                .find(|r| r.tag == b"FRAM")
                .context("animation STAT FRAM missing")?;
            let n = u16le(placement.data, 0)? as usize;
            ensure!(
                n > 0 && n <= 128 && placement.data.len() == 2 + n * 10,
                "invalid animation state sprite count"
            );
            let mut placements = Vec::with_capacity(n);
            for i in 0..n {
                let off = 2 + i * 10;
                let frame = u16le(placement.data, off)? as u32;
                ensure!(
                    (frame as usize) < frames.len(),
                    "animation state refers to nonexistent frame {frame}"
                );
                placements.push(FramePlacement {
                    frame,
                    offset_x: i16le(placement.data, off + 2)? as i32,
                    offset_y: i16le(placement.data, off + 4)? as i32,
                    reserved: [
                        i16le(placement.data, off + 6)?,
                        i16le(placement.data, off + 8)?,
                    ],
                });
            }
            seq.frames.push(placements[0].frame);
            seq.offsets
                .push([placements[0].offset_x, placements[0].offset_y]);
            seq.durations_ms.push(duration_ms);
            seq.states.push(AnimationState {
                duration_raw,
                duration_ms,
                flags: [i16le(header.data, 2)?, i16le(header.data, 4)?],
                parts: placements,
            });
        }
        ensure!(
            seq.states.len() == u16le(head.data, 78)? as usize,
            "animation state count disagrees with sequence header"
        );
        ensure!(!seq.states.is_empty(), "empty animation sequence");
        sequences.push(seq);
    }
    Ok(Animation { frames, sequences })
}

pub(crate) struct BitmapFont {
    pub line_height: u32,
    pub spacing: u32,
    pub glyphs: Vec<(u32, u32, Vec<u8>)>,
}
pub(crate) fn font(data: &[u8]) -> Result<BitmapFont> {
    ensure!(data.len() >= 20, "truncated bitmap font header");
    let count = u32le(data, 0)? as usize;
    let height = u32le(data, 4)? as usize;
    let spacing = u32le(data, 8)?;
    ensure!(
        (1..=256).contains(&count) && (1..=128).contains(&height) && spacing <= 64,
        "invalid bitmap font dimensions"
    );
    let bits = data
        .get(20 + count * 8..)
        .context("truncated bitmap font character table")?;
    let mut glyphs = Vec::with_capacity(count);
    for code in 0..count {
        let w = u32le(data, 20 + code * 8)? as usize;
        let off = u32le(data, 24 + code * 8)? as usize;
        ensure!(w <= 128, "bitmap font glyph too wide");
        let stride = w.div_ceil(8);
        let end = off
            .checked_add(stride * height)
            .context("bitmap font glyph length overflow")?;
        let source = bits
            .get(off..end)
            .context("bitmap font glyph outside file")?;
        let mut rgba = Vec::with_capacity(w * height * 4);
        for row in 0..height {
            for col in 0..w {
                rgba.extend([
                    255,
                    255,
                    255,
                    if source[row * stride + col / 8] & (128 >> (col % 8)) != 0 {
                        255
                    } else {
                        0
                    },
                ]);
            }
        }
        glyphs.push((code as u32, w as u32, rgba));
    }
    Ok(BitmapFont {
        line_height: height as u32,
        spacing,
        glyphs,
    })
}

/// Preserve commands and numeric identifiers in their original order, including
/// duplicate sound entries which the native resource reader can choose between.
pub(crate) fn resource(id: &str, data: &[u8]) -> Result<ResourceTable> {
    let text: String = data.iter().map(|&b| char::from(b)).collect();
    let mut commands = Vec::new();
    for line in text.lines() {
        let line = line.split('\u{1a}').next().unwrap_or_default().trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        let mut quoted = false;
        let mut fields = Vec::new();
        let mut current = String::new();
        for ch in line.chars() {
            match ch {
                '"' => quoted = !quoted,
                ';' if !quoted => break,
                ',' if !quoted => {
                    fields.push(current.trim().to_owned());
                    current.clear();
                }
                _ => current.push(ch),
            }
        }
        fields.push(current.trim().to_owned());
        if !fields[0].is_empty() {
            commands.push(ResourceCommand {
                kind: fields.remove(0),
                fields,
            });
        }
    }
    Ok(ResourceTable {
        id: id.to_owned(),
        commands,
        text,
    })
}

pub(crate) fn scheme(id: &str, data: &[u8]) -> Result<Scheme> {
    let table = resource(id, data)?;
    let mut name = None;
    let mut density = None;
    let mut version = None;
    let mut rows = vec![None; 11];
    let mut spawns = Vec::new();
    let mut powers = Vec::new();
    let mut players = BTreeSet::new();
    let mut kinds = BTreeSet::new();
    for c in &table.commands {
        let n = |i: usize| -> Result<i32> {
            c.fields
                .get(i)
                .context("missing scheme field")?
                .parse()
                .context("invalid scheme number")
        };
        match c.kind.as_str() {
            "-V" => version = Some(n(0)?),
            "-N" => {
                ensure!(name.is_none(), "duplicate scheme name");
                name = Some(c.fields.join(","));
            }
            "-B" => {
                let v = n(0)?;
                ensure!(
                    (0..=100).contains(&v) && density.is_none(),
                    "invalid/duplicate scheme density"
                );
                density = Some(v as u8);
            }
            "-R" => {
                let i = n(0)?;
                ensure!(
                    (0..11).contains(&i) && c.fields.len() == 2,
                    "invalid scheme row number"
                );
                let row = c.fields[1].clone();
                ensure!(
                    row.len() == 15 && row.bytes().all(|b| b"#:.".contains(&b)),
                    "invalid scheme row cells"
                );
                ensure!(
                    rows[i as usize].replace(row).is_none(),
                    "duplicate scheme row"
                );
            }
            "-S" => {
                ensure!(
                    c.fields.len() == 3 || c.fields.len() == 4,
                    "invalid scheme spawn field count"
                );
                let p = n(0)?;
                let x = n(1)?;
                let y = n(2)?;
                let team = if c.fields.len() == 4 {
                    n(3)?
                } else {
                    i32::from(p >= 5)
                };
                ensure!(
                    (0..10).contains(&p)
                        && (0..15).contains(&x)
                        && (0..11).contains(&y)
                        && (0..2).contains(&team),
                    "invalid scheme spawn coordinates/player/team"
                );
                ensure!(players.insert(p), "duplicate scheme player spawn");
                spawns.push(Spawn {
                    player: p as u8,
                    x: x as u8,
                    y: y as u8,
                    team: team as u8,
                });
            }
            "-P" => {
                let index = n(0)?;
                let born = n(1)?;
                let over = n(2)?;
                let value = n(3)?;
                let forbidden = n(4)?;
                ensure!(
                    (0..13).contains(&index)
                        && (0..=255).contains(&born)
                        && [over, forbidden].iter().all(|v| *v == 0 || *v == 1),
                    "invalid scheme powerup fields"
                );
                ensure!(kinds.insert(index), "duplicate scheme powerup");
                powers.push(SchemePowerup {
                    index: index as u8,
                    born_with: born as u32,
                    has_override: over != 0,
                    override_value: value,
                    forbidden: forbidden != 0,
                });
            }
            _ => bail!("unknown scheme command {}", c.kind),
        }
    }
    ensure!(version == Some(2), "unsupported/missing scheme version");
    let rows: Vec<String> = rows
        .into_iter()
        .collect::<Option<_>>()
        .context("scheme must contain all eleven rows")?;
    ensure!(
        players.len() == 10 && kinds.len() == 13,
        "scheme needs ten player starts and thirteen powerup definitions"
    );
    spawns.sort_by_key(|s| s.player);
    powers.sort_by_key(|p| p.index);
    Ok(Scheme {
        id: id.to_owned(),
        name: name.context("missing scheme name")?,
        brick_density: density.context("missing scheme density")?,
        rows,
        spawns,
        powerups: powers,
        text: table.text,
    })
}

pub(crate) fn wav(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        data.len().is_multiple_of(2),
        "RSS audio ends inside a 16-bit sample"
    );
    ensure!(
        data.len() <= u32::MAX as usize - 36,
        "RSS audio too large for RIFF"
    );
    let payload_len = data.len().div_ceil(4) * 4;
    let mut out = Vec::with_capacity(44 + payload_len);
    out.extend(b"RIFF");
    out.extend(((payload_len + 36) as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(22050u32.to_le_bytes());
    out.extend(88200u32.to_le_bytes());
    out.extend(4u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend((payload_len as u32).to_le_bytes());
    out.extend(data);
    out.resize(44 + payload_len, 0);
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn rle16_literal_repeat_and_terminator() {
        assert_eq!(
            rle(&[1, 1, 0, 2, 0, 0x82, 9, 0, 255], 2, 10).unwrap(),
            [1, 0, 2, 0, 9, 0, 9, 0, 9, 0]
        );
        for bad in [
            &[0x80][..],
            &[0, 1][..],
            &[255][..],
            &[0x82, 9, 0, 255][..],
            &[0x80, 1, 0][..],
            &[0x80, 1, 0, 255, 0][..],
        ] {
            assert!(rle(bad, 2, 2).is_err(), "{bad:?}");
        }
    }
    pub(crate) fn pcx_fixture() -> Vec<u8> {
        let mut b = vec![0; 128];
        b[0] = 10;
        b[2] = 1;
        b[3] = 8;
        b[8] = 1;
        b[10] = 1;
        b[65] = 1;
        b[66] = 2;
        b.extend([1, 2, 3, 4, 12]);
        b.resize(b.len() + 768, 0);
        let n = b.len() - 768;
        b[n + 3] = 99;
        b[n + 6 + 1] = 88;
        b[n + 9 + 2] = 77;
        b
    }
    #[test]
    fn pcx_palette_scanline_and_bounds() {
        let b = pcx_fixture();
        let p = pcx(&b).unwrap();
        assert_eq!((p.width, p.height), (2, 2));
        assert_eq!(
            &p.rgba[..12],
            &[99, 0, 0, 255, 0, 88, 0, 255, 0, 0, 77, 255]
        );
        for end in 0..b.len() {
            assert!(pcx(&b[..end]).is_err());
        }
        let mut bad = b.clone();
        bad[128] = 0xc0;
        assert!(pcx(&bad).is_err());
        bad[128] = 0xff;
        assert!(pcx(&bad).is_err());
        bad[66] = 1;
        assert!(pcx(&bad).is_err());
    }
    #[test]
    fn bitmap_font_validates_glyph_ranges() {
        let mut b = vec![0; 29];
        b[0] = 1;
        b[4] = 1;
        b[20] = 1;
        b[28] = 128;
        let f = font(&b).unwrap();
        assert_eq!(f.glyphs[0].2, [255, 255, 255, 255]);
        b[24] = 2;
        assert!(font(&b).is_err());
        for n in 0..28 {
            assert!(font(&b[..n]).is_err());
        }
    }
    #[test]
    fn chunk_and_cimg_boundaries_are_checked() {
        for n in 1..10 {
            assert!(chunks(&vec![0; n]).is_err());
        }
        let mut b = vec![0; 10];
        b[4] = 1;
        assert!(chunks(&b).is_err());
        let p = Palette {
            rgb: vec![[0; 3]; 256],
            lookup: vec![1; 32768],
        };
        let mut b = vec![0; 39];
        b[0] = 4;
        b[2] = 4;
        b[4] = 24;
        b[12] = 1;
        b[14] = 1;
        b[24] = 0x11;
        b[26] = 12;
        b[28] = 15;
        b[32] = 2;
        b[36] = 0x80;
        b[37] = 1;
        b.push(255);
        b[28] = 16;
        assert_eq!(cimg(&b, &p).unwrap().indices, [1]);
        for n in 0..b.len() {
            assert!(cimg(&b[..n], &p).is_err());
        }
        b[4] = 255;
        assert!(cimg(&b, &p).is_err());
    }
    #[test]
    fn signed_stereo_audio_is_preserved_exactly() {
        let raw = [0, 128, 255, 127, 255, 255, 1, 0];
        let w = wav(&raw).unwrap();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32le(&w, 24).unwrap(), 22050);
        assert_eq!(&w[44..], &raw);
        for n in [1, 3, 5, 7] {
            assert!(wav(&raw[..n]).is_err());
        }
        let padded = wav(&raw[..6]).unwrap();
        assert_eq!(&padded[44..50], &raw[..6]);
        assert_eq!(&padded[50..], &[0, 0]);
    }
    #[test]
    fn resource_retains_duplicate_numbers_and_quoted_commas() {
        let t = resource(
            "test",
            b"; comment\n200,foo ; note\n200,bar\n5,\"x,y\"\n\x1a",
        )
        .unwrap();
        assert_eq!(t.commands.len(), 3);
        assert_eq!(t.commands[2].fields, ["x,y"]);
        assert_eq!(t.commands[0].kind, "200");
    }
    pub(crate) fn scheme_fixture() -> String {
        let mut s = String::from("-V,2\n-N,Fixture arena\n-B,90\n");
        for row in 0..11 {
            s.push_str(&format!("-R,{row},...............\n"));
        }
        for p in 0..10 {
            s.push_str(&format!("-S,{p},{p},0\n"));
        }
        for p in 0..13 {
            s.push_str(&format!("-P,{p},0,0,0,0,test\n"));
        }
        s
    }
    #[test]
    fn scheme_accepts_legacy_teams_and_rejects_incomplete_or_malformed_fields() {
        let raw = scheme_fixture();
        let s = scheme("FIXTURE.SCH", raw.as_bytes()).unwrap();
        assert_eq!(s.rows.len(), 11);
        assert_eq!(s.spawns[0].team, 0);
        assert_eq!(s.spawns[9].team, 1);
        let mut team = raw.replace("-S,0,0,0", "-S,0,0,0,1");
        assert_eq!(
            scheme("fixture", team.as_bytes()).unwrap().spawns[0].team,
            1
        );
        for (from, to) in [
            ("-B,90", "-B,101"),
            ("-S,0,0,0", "-S,0,15,0"),
            ("-P,0,0,0,0,0,test", "-P,13,0,0,0,0,test"),
            ("-R,10,...............\n", ""),
            ("-V,2", "-V,9"),
        ] {
            team = raw.replace(from, to);
            assert!(scheme("fixture", team.as_bytes()).is_err(), "{from}");
        }
        assert!(scheme("fixture", format!("{raw}-R,0,...............\n").as_bytes()).is_err());
    }
    fn chunk(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut b = tag.to_vec();
        b.extend((data.len() as u32).to_le_bytes());
        b.extend([0, 0]);
        b.extend(data);
        b
    }
    pub(crate) fn animation_fixture() -> Vec<u8> {
        let mut image = vec![0; 38];
        image[0] = 4;
        image[2] = 4;
        image[4] = 24;
        image[12] = 1;
        image[14] = 1;
        image[26] = 12;
        image[28] = 14;
        image[32] = 2;
        image[36] = 1;
        let frame = chunk(b"FRAM", &chunk(b"CIMG", &image));
        let mut sequence_head = vec![0; 96];
        sequence_head[..4].copy_from_slice(b"walk");
        sequence_head[78] = 2;
        let mut seq = chunk(b"HEAD", &sequence_head);
        for duration in [30i16, -1] {
            let mut head = vec![0; 46];
            head[..2].copy_from_slice(&duration.to_le_bytes());
            let mut state = chunk(b"HEAD", &head);
            state.extend(chunk(b"FRAM", &[1, 0, 0, 0, 2, 0, 3, 0, 0, 0, 0, 0]));
            seq.extend(chunk(b"STAT", &state));
        }
        let mut body = frame;
        body.extend(chunk(b"SEQ ", &seq));
        let mut b = b"CHFILEANI ".to_vec();
        b.extend((body.len() as u32).to_le_bytes());
        b.extend([0, 0]);
        b.extend(body);
        b
    }
    #[test]
    fn animation_keeps_canvas_origins_sequence_offsets_and_inherited_durations() {
        let p = Palette {
            rgb: vec![[0; 3]; 256],
            lookup: vec![1; 32768],
        };
        let b = animation_fixture();
        let a = animation(&b, &p).unwrap();
        assert_eq!(a.frames.len(), 1);
        assert_eq!(a.frames[0].indices, [1]);
        assert_eq!(a.sequences[0].durations_ms, [30, 30]);
        assert_eq!(a.sequences[0].offsets, [[2, 3], [2, 3]]);
        assert_eq!(a.sequences[0].states[1].duration_raw, -1);
        for n in 0..b.len() {
            assert!(animation(&b[..n], &p).is_err());
        }
        let mut bad = b.clone();
        let pos = bad
            .windows(12)
            .position(|w| w == [1, 0, 0, 0, 2, 0, 3, 0, 0, 0, 0, 0])
            .unwrap();
        bad[pos + 2] = 1;
        assert!(animation(&bad, &p).is_err());
    }
}

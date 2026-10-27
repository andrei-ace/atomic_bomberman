use anyhow::{ensure, Context, Result};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

const SECTOR_DATA: usize = 2048;
const MAX_FILE: u64 = 128 * 1024 * 1024;
const MAX_FILES: usize = 50_000;

pub(crate) enum Source {
    Disc(Disc),
    Directory { files: BTreeMap<String, PathBuf> },
}

impl Source {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        ensure!(
            !fs::symlink_metadata(path)?.file_type().is_symlink(),
            "source must not be a symlink"
        );
        if path.is_dir() {
            let mut files = BTreeMap::new();
            walk(path, path, 0, &mut files)?;
            ensure!(
                files.contains_key("COLOR.PAL"),
                "extracted game directory must contain COLOR.PAL at its root"
            );
            Ok(Self::Directory { files })
        } else {
            Ok(Self::Disc(Disc::open(path)?))
        }
    }
    pub(crate) fn paths(&self) -> Vec<String> {
        match self {
            Self::Disc(d) => d.files.keys().cloned().collect(),
            Self::Directory { files, .. } => files.keys().cloned().collect(),
        }
    }
    pub(crate) fn read(&mut self, path: &str) -> Result<Vec<u8>> {
        match self {
            Self::Disc(d) => {
                let (extent, len) = *d
                    .files
                    .get(path)
                    .with_context(|| format!("missing {path}"))?;
                d.read_extent(extent, len)
            }
            Self::Directory { files, .. } => {
                let p = files.get(path).with_context(|| format!("missing {path}"))?;
                ensure!(
                    fs::metadata(p)?.len() <= MAX_FILE,
                    "asset too large: {path}"
                );
                fs::read(p).with_context(|| format!("read {path}"))
            }
        }
    }
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<()> {
    ensure!(depth <= 32, "source directory nesting exceeds 32");
    for e in fs::read_dir(dir)? {
        let e = e?;
        let ty = e.file_type()?;
        ensure!(
            !ty.is_symlink(),
            "source contains symlink: {}",
            e.path().display()
        );
        if ty.is_dir() {
            walk(root, &e.path(), depth + 1, files)?;
        } else if ty.is_file() {
            let p = e.path();
            let relative = p
                .strip_prefix(root)?
                .to_str()
                .context("source path is not UTF-8")?;
            check_relative(relative)?;
            let name = relative.to_ascii_uppercase();
            ensure!(
                files.insert(name.clone(), p).is_none(),
                "case-insensitive duplicate source path: {name}"
            );
            ensure!(files.len() <= MAX_FILES, "source has too many files");
        }
    }
    Ok(())
}

pub(crate) fn check_relative(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && !path.contains(['\\', ':', '\0']),
        "unsafe relative path {path:?}"
    );
    for c in Path::new(path).components() {
        ensure!(
            matches!(c, Component::Normal(_)),
            "unsafe relative path {path:?}"
        );
    }
    // Components normalizes repeated separators; reject them explicitly as well.
    ensure!(
        path.split('/')
            .all(|s| !s.is_empty() && s != "." && s != ".."),
        "unsafe relative path {path:?}"
    );
    Ok(())
}

pub(crate) struct Disc {
    file: File,
    sector_bytes: u64,
    payload_offset: u64,
    sectors: u64,
    files: BTreeMap<String, (u32, u32)>,
}

impl Disc {
    fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let size = file.metadata()?.len();
        let mut detected = None;
        for (sector, offset) in [(2352, 16), (2048, 0)] {
            if size < 17 * sector {
                continue;
            }
            file.seek(SeekFrom::Start(16 * sector + offset))?;
            let mut header = [0; 7];
            file.read_exact(&mut header)?;
            if &header[1..6] == b"CD001" && header[6] == 1 {
                detected = Some((sector, offset));
                break;
            }
        }
        let (sector_bytes, payload_offset) =
            detected.context("source is not a MODE1/2352 or 2048-byte ISO9660 image")?;
        ensure!(size % sector_bytes == 0, "truncated CD image sector");
        let mut disc = Self {
            file,
            sector_bytes,
            payload_offset,
            sectors: size / sector_bytes,
            files: BTreeMap::new(),
        };
        let mut root = None;
        for lba in 16..80 {
            let pvd = disc.read_extent(lba, 2048)?;
            ensure!(
                &pvd[1..6] == b"CD001" && pvd[6] == 1,
                "invalid ISO volume descriptor"
            );
            if pvd[0] == 1 {
                ensure!(
                    both_u16(&pvd, 128)? == 2048,
                    "unsupported ISO logical block size"
                );
                ensure!(
                    u64::from(both_u32(&pvd, 80)?) <= disc.sectors,
                    "ISO volume exceeds source length"
                );
                let rec = directory_record(&pvd[156..])?;
                ensure!(rec.directory, "ISO root is not a directory");
                root = Some((rec.extent, rec.length));
                break;
            }
            if pvd[0] == 255 {
                break;
            }
        }
        let (extent, length) = root.context("ISO primary volume descriptor missing")?;
        disc.walk_directory("", extent, length, 0, &mut HashSet::new())?;
        ensure!(
            disc.files.contains_key("COLOR.PAL"),
            "CD image has no game COLOR.PAL at its root"
        );
        Ok(disc)
    }

    fn read_extent(&mut self, lba: u32, length: u32) -> Result<Vec<u8>> {
        ensure!(u64::from(length) <= MAX_FILE, "ISO extent is too large");
        let blocks = u64::from(length).div_ceil(SECTOR_DATA as u64);
        ensure!(
            u64::from(lba)
                .checked_add(blocks)
                .is_some_and(|n| n <= self.sectors),
            "ISO extent outside CD image"
        );
        let mut out = vec![0; length as usize];
        for (i, chunk) in out.chunks_mut(SECTOR_DATA).enumerate() {
            let sector_start = (u64::from(lba) + i as u64) * self.sector_bytes;
            if self.payload_offset != 0 {
                self.file.seek(SeekFrom::Start(sector_start))?;
                let mut h = [0; 16];
                self.file.read_exact(&mut h)?;
                ensure!(
                    h[0] == 0 && h[1..11] == [255; 10] && h[11] == 0 && h[15] == 1,
                    "non-MODE1 sector at LBA {}",
                    u64::from(lba) + i as u64
                );
            } else {
                self.file.seek(SeekFrom::Start(sector_start))?;
            }
            self.file
                .read_exact(chunk)
                .context("truncated ISO payload")?;
        }
        Ok(out)
    }

    fn walk_directory(
        &mut self,
        parent: &str,
        extent: u32,
        length: u32,
        depth: usize,
        seen: &mut HashSet<u32>,
    ) -> Result<()> {
        ensure!(depth <= 32, "ISO directory nesting exceeds 32");
        ensure!(
            seen.insert(extent),
            "ISO cyclic or aliased directory extent {extent}"
        );
        ensure!(length <= 16 * 1024 * 1024, "ISO directory too large");
        let data = self.read_extent(extent, length)?;
        let mut offset = 0;
        while offset < data.len() {
            let n = data[offset] as usize;
            if n == 0 {
                offset = ((offset / SECTOR_DATA) + 1) * SECTOR_DATA;
                continue;
            }
            ensure!(
                (offset % SECTOR_DATA) + n <= SECTOR_DATA,
                "ISO directory record crosses sector"
            );
            ensure!(offset + n <= data.len(), "truncated ISO directory record");
            let rec = directory_record(&data[offset..offset + n])?;
            offset += n;
            if rec.name == [0] || rec.name == [1] {
                continue;
            }
            let raw = std::str::from_utf8(rec.name).context("non-ASCII ISO identifier")?;
            ensure!(raw.is_ascii(), "non-ASCII ISO identifier");
            let raw = raw
                .split_once(';')
                .map_or(raw, |(n, _)| n)
                .trim_end_matches('.');
            ensure!(!raw.contains('/'), "ISO filename contains separator");
            check_relative(raw)?;
            let name = if parent.is_empty() {
                raw.to_ascii_uppercase()
            } else {
                format!("{parent}/{}", raw.to_ascii_uppercase())
            };
            if rec.directory {
                self.walk_directory(&name, rec.extent, rec.length, depth + 1, seen)?;
            } else {
                ensure!(
                    u64::from(rec.length) <= MAX_FILE,
                    "ISO file too large: {name}"
                );
                ensure!(
                    u64::from(rec.extent) + u64::from(rec.length).div_ceil(2048) <= self.sectors,
                    "ISO file outside image: {name}"
                );
                ensure!(
                    self.files
                        .insert(name.clone(), (rec.extent, rec.length))
                        .is_none(),
                    "duplicate ISO path {name}"
                );
                ensure!(self.files.len() <= MAX_FILES, "too many ISO files");
            }
        }
        Ok(())
    }
}

struct Record<'a> {
    extent: u32,
    length: u32,
    directory: bool,
    name: &'a [u8],
}

fn directory_record(data: &[u8]) -> Result<Record<'_>> {
    ensure!(data.len() >= 34, "truncated ISO directory record");
    let n = data[0] as usize;
    ensure!(
        n >= 34 && n <= data.len(),
        "invalid ISO directory record length"
    );
    ensure!(
        data[1] == 0 && data[26] == 0 && data[27] == 0,
        "unsupported ISO extended attributes/interleaving"
    );
    ensure!(
        data[25] & 0x80 == 0,
        "ISO multi-extent files are unsupported"
    );
    let name_len = data[32] as usize;
    ensure!(
        name_len > 0 && 33 + name_len <= n,
        "truncated ISO identifier"
    );
    Ok(Record {
        extent: both_u32(data, 2)?,
        length: both_u32(data, 10)?,
        directory: data[25] & 2 != 0,
        name: &data[33..33 + name_len],
    })
}

fn both_u32(data: &[u8], off: usize) -> Result<u32> {
    let b = data
        .get(off..off + 8)
        .context("truncated ISO both-endian integer")?;
    let le = u32::from_le_bytes(b[..4].try_into()?);
    ensure!(
        le == u32::from_be_bytes(b[4..].try_into()?),
        "ISO integer endian copies disagree"
    );
    Ok(le)
}
fn both_u16(data: &[u8], off: usize) -> Result<u16> {
    let b = data
        .get(off..off + 4)
        .context("truncated ISO both-endian integer")?;
    let le = u16::from_le_bytes(b[..2].try_into()?);
    ensure!(
        le == u16::from_be_bytes(b[2..].try_into()?),
        "ISO integer endian copies disagree"
    );
    Ok(le)
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
                "bomberman-iso-test-{}-{}",
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
    fn put_both32(data: &mut [u8], off: usize, value: u32) {
        data[off..off + 4].copy_from_slice(&value.to_le_bytes());
        data[off + 4..off + 8].copy_from_slice(&value.to_be_bytes());
    }
    fn record(name: &[u8], extent: u32, length: u32, directory: bool) -> Vec<u8> {
        let n = (33 + name.len() + 1) & !1;
        let mut r = vec![0; n];
        r[0] = n as u8;
        put_both32(&mut r, 2, extent);
        put_both32(&mut r, 10, length);
        r[25] = if directory { 2 } else { 0 };
        r[32] = name.len() as u8;
        r[33..33 + name.len()].copy_from_slice(name);
        r
    }
    fn iso_fixture(filename: &[u8], extent: u32, directory: bool) -> Vec<u8> {
        let mut iso = vec![0; 32 * 2048];
        let pvd = &mut iso[16 * 2048..17 * 2048];
        pvd[0] = 1;
        pvd[1..6].copy_from_slice(b"CD001");
        pvd[6] = 1;
        put_both32(pvd, 80, 32);
        pvd[128..132].copy_from_slice(&[0, 8, 8, 0]);
        let root = record(&[0], 18, 2048, true);
        pvd[156..156 + root.len()].copy_from_slice(&root);
        let mut dir = root;
        dir.extend(record(&[1], 18, 2048, true));
        dir.extend(record(filename, extent, 3, directory));
        iso[18 * 2048..18 * 2048 + dir.len()].copy_from_slice(&dir);
        iso[19 * 2048..19 * 2048 + 3].copy_from_slice(b"abc");
        iso
    }
    fn raw_mode1(iso: &[u8]) -> Vec<u8> {
        let mut raw = Vec::new();
        for payload in iso.chunks_exact(2048) {
            let mut sector = vec![0; 2352];
            sector[1..11].fill(255);
            sector[15] = 1;
            sector[16..2064].copy_from_slice(payload);
            raw.extend(sector);
        }
        raw
    }
    #[test]
    fn rejects_path_traversal_and_windows_paths() {
        for p in [
            "",
            "../escape",
            "a/../b",
            "/root",
            "C:/a",
            "a\\b",
            "a//b",
            "./a",
            "a\0b",
        ] {
            assert!(check_relative(p).is_err(), "{p}");
        }
        assert!(check_relative("DATA/ANI/WALK.ANI").is_ok());
    }
    #[test]
    fn rejects_truncated_records_and_conflicting_endian_numbers() {
        for n in 0..34 {
            assert!(directory_record(&vec![0; n]).is_err());
        }
        let mut r = vec![0u8; 34];
        r[0] = 34;
        r[32] = 1;
        r[2] = 1;
        assert!(directory_record(&r).is_err());
        r[9] = 1;
        assert!(directory_record(&r).is_ok());
        r[25] = 0x80;
        assert!(directory_record(&r).is_err());
    }
    #[test]
    fn reads_iso_and_mode1_images_without_modifying_source() {
        let t = Temp::new();
        let iso = iso_fixture(b"COLOR.PAL;1", 19, false);
        for (name, bytes) in [("data.iso", iso.clone()), ("data.img", raw_mode1(&iso))] {
            let p = t.0.join(name);
            fs::write(&p, &bytes).unwrap();
            let mut disc = Disc::open(&p).unwrap();
            assert_eq!(disc.files.len(), 1);
            let (lba, n) = disc.files["COLOR.PAL"];
            assert_eq!(disc.read_extent(lba, n).unwrap(), b"abc");
            assert_eq!(fs::read(p).unwrap(), bytes);
        }
    }
    #[test]
    fn rejects_unsafe_iso_identifiers_outside_extents_cycles_and_truncation() {
        let t = Temp::new();
        let p = t.0.join("bad.img");
        for iso in [
            iso_fixture(b"A/../B;1", 19, false),
            iso_fixture(b"COLOR.PAL;1", 40, false),
            iso_fixture(b"LOOP", 18, true),
        ] {
            fs::write(&p, raw_mode1(&iso)).unwrap();
            assert!(Disc::open(&p).is_err());
        }
        let mut raw = raw_mode1(&iso_fixture(b"COLOR.PAL;1", 19, false));
        raw.pop();
        fs::write(&p, &raw).unwrap();
        assert!(Disc::open(&p).is_err());
        raw.push(0);
        raw[18 * 2352 + 15] = 2;
        fs::write(&p, &raw).unwrap();
        assert!(Disc::open(&p).is_err());
    }
    #[test]
    fn directory_input_handles_case_and_rejects_ambiguous_names_and_symlinks() {
        let t = Temp::new();
        fs::write(t.0.join("color.pal"), b"palette").unwrap();
        let mut source = Source::open(&t.0).unwrap();
        assert_eq!(source.read("COLOR.PAL").unwrap(), b"palette");
        fs::write(t.0.join("COLOR.PAL"), b"ambiguous").unwrap();
        assert!(Source::open(&t.0).is_err());
        fs::remove_file(t.0.join("COLOR.PAL")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("color.pal", t.0.join("link")).unwrap();
            assert!(Source::open(&t.0).is_err());
        }
    }
}

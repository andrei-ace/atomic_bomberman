use crate::{Powerup, TilePos, BOARD_HEIGHT, BOARD_WIDTH, MAX_PLAYERS};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnPoint {
    pub player: usize,
    pub tile: TilePos,
    pub team: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemePowerup {
    pub kind: Powerup,
    pub born_with: u8,
    pub override_count: Option<i16>,
    pub forbidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scheme {
    pub name: String,
    pub brick_density: u8,
    pub rows: Vec<String>,
    pub starts: Vec<SpawnPoint>,
    pub powerups: Vec<SchemePowerup>,
}
impl Default for Scheme {
    fn default() -> Self {
        let starts = [
            (0, 0),
            (14, 10),
            (0, 10),
            (14, 0),
            (6, 4),
            (8, 0),
            (12, 4),
            (2, 6),
            (10, 8),
            (6, 10),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (x, y))| SpawnPoint {
            player: i,
            tile: TilePos::new(x, y),
            team: (i % 2) as u8,
        })
        .collect();
        Self {
            name: "Just the BASIC SET! (10)".into(),
            brick_density: 90,
            rows: (0..BOARD_HEIGHT)
                .map(|y| {
                    (0..BOARD_WIDTH)
                        .map(|x| if x % 2 == 1 && y % 2 == 1 { '#' } else { ':' })
                        .collect()
                })
                .collect(),
            starts,
            powerups: Powerup::ALL
                .into_iter()
                .map(|kind| SchemePowerup {
                    kind,
                    born_with: 0,
                    override_count: None,
                    forbidden: false,
                })
                .collect(),
        }
    }
}

impl Scheme {
    pub fn parse(text: &str) -> Result<Self, SchemeError> {
        let mut scheme = Self {
            name: String::new(),
            brick_density: 90,
            rows: vec![String::new(); BOARD_HEIGHT as usize],
            starts: vec![],
            powerups: vec![],
        };
        let mut has_version = false;
        for (line_no, line) in text.trim_end_matches('\u{1a}').lines().enumerate() {
            let line = line.split(';').next().unwrap_or_default().trim();
            if line.is_empty() || line == "\u{1a}" {
                continue;
            }
            let fields: Vec<_> = line.split(',').map(str::trim).collect();
            let fail = |why: &str| SchemeError(format!("scheme line {}: {why}", line_no + 1));
            let int = |index: usize| -> Result<i32, SchemeError> {
                fields
                    .get(index)
                    .ok_or_else(|| fail("missing field"))?
                    .parse()
                    .map_err(|_| fail("expected integer"))
            };
            let bounded = |index: usize, low: i32, high: i32| -> Result<i32, SchemeError> {
                let v = int(index)?;
                if !(low..=high).contains(&v) {
                    Err(fail("field is out of range"))
                } else {
                    Ok(v)
                }
            };
            match fields[0].to_ascii_uppercase().as_str() {
                "-V" => {
                    if int(1)? != 2 {
                        return Err(fail("only version 2 is supported"));
                    }
                    has_version = true;
                }
                "-N" => {
                    scheme.name = line
                        .split_once(',')
                        .ok_or_else(|| fail("missing scheme name"))?
                        .1
                        .trim()
                        .into();
                }
                "-B" => scheme.brick_density = bounded(1, 0, 100)? as u8,
                "-R" => {
                    let y = bounded(1, 0, BOARD_HEIGHT - 1)? as usize;
                    let row = *fields.get(2).ok_or_else(|| fail("missing row data"))?;
                    if !scheme.rows[y].is_empty() {
                        return Err(fail("duplicate board row"));
                    }
                    if row.len() != BOARD_WIDTH as usize
                        || !row.bytes().all(|b| matches!(b, b'#' | b':' | b'.'))
                    {
                        return Err(fail("row must have fifteen #, : or . cells"));
                    }
                    scheme.rows[y] = row.into();
                }
                "-S" => {
                    let player = bounded(1, 0, MAX_PLAYERS as i32 - 1)? as usize;
                    if scheme.starts.iter().any(|s| s.player == player) {
                        return Err(fail("duplicate player start"));
                    }
                    let x = bounded(2, 0, BOARD_WIDTH - 1)?;
                    let y = bounded(3, 0, BOARD_HEIGHT - 1)?;
                    let team = if fields.len() > 4 {
                        bounded(4, 0, 1)? as u8
                    } else {
                        u8::from(player >= 5)
                    };
                    scheme.starts.push(SpawnPoint {
                        player,
                        tile: TilePos::new(x, y),
                        team,
                    });
                }
                "-P" => {
                    let kind = Powerup::from_id(bounded(1, 0, 12)? as u8).unwrap();
                    if scheme.powerups.iter().any(|p| p.kind == kind) {
                        return Err(fail("duplicate powerup definition"));
                    }
                    let born_with = bounded(2, 0, 255)? as u8;
                    let has_override = bounded(3, 0, 1)? != 0;
                    let count = bounded(4, -32768, 32767)? as i16;
                    let forbidden = bounded(5, 0, 1)? != 0;
                    scheme.powerups.push(SchemePowerup {
                        kind,
                        born_with,
                        override_count: has_override.then_some(count),
                        forbidden,
                    });
                }
                _ => return Err(fail("unknown scheme directive")),
            }
        }
        if !has_version {
            return Err(SchemeError("scheme lacks -V,2 version marker".into()));
        }
        scheme.validate()?;
        Ok(scheme)
    }

    pub fn validate(&self) -> Result<(), SchemeError> {
        if self.name.is_empty() || self.name.len() > 512 {
            return Err(SchemeError("invalid scheme name".into()));
        }
        if self.brick_density > 100 {
            return Err(SchemeError("brick density exceeds 100".into()));
        }
        if self.rows.len() != BOARD_HEIGHT as usize
            || self.rows.iter().any(|r| {
                r.len() != BOARD_WIDTH as usize
                    || !r.bytes().all(|b| matches!(b, b'#' | b':' | b'.'))
            })
        {
            return Err(SchemeError(
                "scheme requires eleven rows of fifteen #, : or . cells".into(),
            ));
        }
        if self.starts.len() > MAX_PLAYERS
            || self
                .starts
                .iter()
                .any(|s| s.player >= MAX_PLAYERS || !s.tile.valid())
        {
            return Err(SchemeError("invalid start position".into()));
        }
        for (i, start) in self.starts.iter().enumerate() {
            // Several shipped four-player schemes intentionally reuse the
            // same tile for unused player slots. Preserve those definitions.
            if self.starts[..i].iter().any(|s| s.player == start.player) {
                return Err(SchemeError("duplicate player start definition".into()));
            }
            if self.rows[start.tile.y as usize].as_bytes()[start.tile.x as usize] == b'#' {
                return Err(SchemeError(
                    "start position is inside an indestructible wall".into(),
                ));
            }
        }
        for (i, power) in self.powerups.iter().enumerate() {
            if power.kind == Powerup::Brake
                || self.powerups[..i].iter().any(|p| p.kind == power.kind)
            {
                return Err(SchemeError("invalid or repeated scheme powerup".into()));
            }
        }
        Ok(())
    }
    pub fn powerup(&self, kind: Powerup) -> Option<&SchemePowerup> {
        self.powerups.iter().find(|p| p.kind == kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemeError(pub String);
impl fmt::Display for SchemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SchemeError {}

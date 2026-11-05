//! Original score colors come from RMP percentages and COLOR.PAL's RGB555 table.
use super::*;

// The supplied English disc's RMP bytes are identified by their full source
// digest. This recovers precise profiles for older stock caches without
// interpreting arbitrary custom remaps as the same color. See the native
// 0x414a65/0x41672f audit in tools/compare_hud_layout.py.
const STOCK_PROFILES: [(&str, [u8; 3]); 10] = [
    (
        "721a5cb42ceb42eea427eb6b90cce1d575e4052f764d7fafca14672dd164a6ae",
        [100, 100, 100],
    ),
    (
        "5e2a7d2fd62855322d7a928c2b48d2451106ca866b95441133fafcc8466e12bb",
        [15, 15, 10],
    ),
    (
        "94cd05eb100be277ab5d8e63c5aa1e5313a707bbba1fb357ff28ce3f30b30f53",
        [100, 0, 10],
    ),
    (
        "e95e16d41532d29165230cbcd0270eac116214b74c3f2261b2fa727aad1b2f89",
        [0, 10, 85],
    ),
    (
        "3a6b4e5513283ecee83af24e798812485a87b28fbadb3baed48d08c1b8a1c8cc",
        [0, 100, 0],
    ),
    (
        "0192489cd510948f8cb814902f462748bad4ac7484f0a1341105bd265cd8bba6",
        [100, 100, 0],
    ),
    (
        "1539f37eaedaa3765c0091ca96aa50308c22ae74ace8380cb89b14e86ea226ef",
        [0, 90, 95],
    ),
    (
        "f8bdfb7467b294088993ca9a5f983f426fcc49b6377686e8e566e253ec9752e6",
        [95, 0, 90],
    ),
    (
        "6470cd7a3255e4f136f0e423b179ea84bbf194959071d7c39e6e8f7c6bd5452b",
        [100, 50, 0],
    ),
    (
        "267b4d52eae8d8cb078b03e48a61e3b03cb3a340711115728dbe8a7026cf0c99",
        [50, 0, 100],
    ),
];

pub(super) fn remap_profile(data: &[u8]) -> Option<[u8; 3]> {
    data.get(256..259).map(|rgb| [rgb[0], rgb[1], rgb[2]])
}

pub(super) fn resource_profile(manifest: &Manifest, player: usize) -> Result<[u8; 3]> {
    let table = manifest
        .resources
        .iter()
        .find(|r| r.id == "DATA/RES/VALUELST.RES")
        .context("score color fallback needs VALUELST.RES")?;
    let mut rgb = [0; 3];
    for (channel, value) in rgb.iter_mut().enumerate() {
        let key = (200 + player * 5 + channel).to_string();
        *value = table
            .commands
            .iter()
            .rev()
            .find(|c| c.kind == key)
            .and_then(|c| c.fields.first())
            .with_context(|| format!("score color fallback missing VALUELST.RES {key}"))?
            .parse()
            .with_context(|| format!("invalid score color percentage VALUELST.RES {key}"))?;
    }
    Ok(rgb)
}

pub(super) fn base_profiles(manifest: &Manifest) -> Vec<Option<[u8; 3]>> {
    if manifest.hud_color_percentages.len() == 10 {
        return manifest
            .hud_color_percentages
            .iter()
            .copied()
            .map(Some)
            .collect();
    }
    STOCK_PROFILES
        .iter()
        .enumerate()
        .map(|(player, (digest, rgb))| {
            manifest
                .sources
                .iter()
                .any(|s| s.path == format!("{player}.RMP") && s.sha256 == *digest)
                .then_some(*rgb)
        })
        .collect()
}

pub(super) fn finish_profiles(
    manifest: &mut Manifest,
    profiles: Vec<Option<[u8; 3]>>,
) -> Result<()> {
    let mut percentages = Vec::with_capacity(10);
    for (player, profile) in profiles.into_iter().enumerate() {
        let id = format!("{player}.RMP");
        let resource_fallback = manifest
            .sources
            .iter()
            .any(|s| s.path == id && s.bytes == 256);
        percentages.push(if resource_fallback {
            resource_profile(manifest, player)?
        } else {
            profile.with_context(|| format!(
                "this older cache lacks score color percentages for {id}; include that original RMP in the mod or reimport the base game once"
            ))?
        });
    }
    ensure!(percentages.len() == 10, "expected ten score color profiles");
    ensure!(manifest.palette_lookup.len() == 32768,
        "score color overrides need COLOR.PAL with this older base cache; include the original COLOR.PAL in the mod or reimport the base game once");
    manifest.hud_color_indices = percentages
        .iter()
        .map(|rgb| {
            let [r, g, b] = rgb.map(|v| usize::from((v / 3).min(31)));
            manifest.palette_lookup[r << 10 | g << 5 | b]
        })
        .collect();
    manifest.hud_color_percentages = percentages;
    Ok(())
}

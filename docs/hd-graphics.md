# Optional HD graphics

Classic is the default for new and existing profiles. Options → Graphics
contains the graphics selector, Fullscreen, Prepare/enable HD (or Retry), Use
existing HD folder, Export HD artwork, and Back. The first HD selection prepares artwork locally; Escape or Enter cancels
the preparation. Select Classic to return to the original artwork.
Switching to Classic preserves the completed HD cache on disk. Selecting HD
again validates and reuses that cache for the same game assets or mod, without
running enhancement again. Voice-only and scheme-only changes also reuse it.

The source cache is opened read-only. HD output lives in `hd-cache` beside the
selected settings file. Its identity includes the active asset manifest, mod
visual content, palette remaps, scale, model and enhancement format version.
Voice-only and scheme-only mods reuse identical HD artwork; changed graphics,
fonts, animation metadata or remaps need their own matching cache. Completed caches are validated and
published atomically; a cancelled/failed build never replaces a working cache.

Preparation downloads the official Real-ESRGAN v0.2.5.0 / 20220424 native helper
and models (roughly 45–50 MB, depending on platform). Archive and extracted-file
SHA-256 values are pinned in the source. Only the executable, required Windows
OpenMP runtime, `realesrgan-x4plus` model and model parameters are extracted.
The tool cache includes the upstream license notices. Original game data is
never uploaded.

Cache checks use runtime-detected ARM SHA acceleration on supported AArch64
CPUs, with the RustCrypto software fallback on other ARM devices. This preserves
the existing checksums and cache identity.

The helper is universal Intel/Apple Silicon on macOS and x86-64 on Windows/Linux.
macOS uses its bundled Vulkan-to-Metal implementation; Windows/Linux need a
Vulkan-capable GPU driver. Unsupported systems keep Classic. Preparation runs
in background workers, with a separate cancellable inference process for each
active asset group. A cross-process lock prevents concurrent windows from
writing the same tool/cache generation.

## Preparation performance

On a **MacBook Pro with an Apple M4 Pro (14 CPU cores, 48 GB RAM)**, a complete
format-2 preparation took **59 minutes 36 seconds** with **two automatically
selected workers**. This measured run processed all **16,183 frames**, including
ten player colors, into **1,174 PNG pages**. It includes source checks,
approximately 17 seconds of first-use calibration and final cache validation.
The helper was already downloaded. Source builds and native HUD checks ran
during preparation, so this represents a normal development session rather
than an isolated GPU benchmark.

A separate 93-frame comparison took **25.45 seconds serially** and **20.90 seconds
with two workers**, an approximately **18% time reduction** with identical page
hashes and frame mappings. These batch timings exclude their separate
23-second calibration; they do not compare two complete stock runs.

Times vary with the MacBook's GPU, available RAM, thermal conditions and other
running work. The model download is approximately **50 MB**, and the completed
stock HD pages occupy approximately **1.36 GB**. Later launches and switches
between Classic and HD reuse the matching completed cache. A visual mod that
changes the source artwork needs its own matching HD cache.

The worker limit follows available CPU threads, available RAM and the GPU
working-set budget where reported (Metal on macOS). It reserves capacity for
the game and OS, budgets at least 1 GiB of RAM per worker, and caps concurrency
at four. Systems without a reported GPU budget use a conservative two-worker
cap. Each asset also has a checked memory limit before its images are decoded.

A short first-use calibration compares small batches at different worker
counts within those limits. It favors fewer workers when timings are within
5%, because more GPU contexts can reduce throughput. When calibration runs, its
chosen count and timings are saved locally in `preparation.json` inside the new
pack. Existing valid HD caches are reused immediately.

Workers use separate temporary directories and stable page names. Parallelism
does not change the model, scale, palette processing or cache identity. Available
RAM is checked during preparation; new work is reduced when memory becomes
tight. If a GPU helper fails during parallel preparation, all helpers are joined
before unfinished assets receive one retry with a single worker. Completed
assets from that attempt are retained. A further failure returns to Classic.
Cancel terminates and reaps every helper and removes the staged generation.

## Transfer HD artwork between computers

On a computer with completed HD artwork, select **Options → Graphics → Export
HD artwork** and choose a destination directory. The app creates an **Atomic
Bomberman HD** folder there, containing the matching enhanced pages, manifest
and instructions. Export also works while Classic is selected. Existing
unrelated folders are preserved; choose a different destination if that name
is already occupied by another export.
The complete stock pack contains 16,183 frames packed into 1,174 PNG sprite
sheets, plus a manifest mapping each frame to its rectangle. All sheets are
transferred; the smaller file count does not omit frames.

Copy the exported folder to the other computer. Import its original game data
as usual, activate the matching mod if applicable, then select **Options →
Graphics → Use existing HD folder**. Choose the exported folder, its parent,
or an existing game's `hd-cache` directory. All original arenas, fonts, frames,
palette variants, enhancement version and source identity must match.

Import validates the page geometry, hashes and decoded image data, then copies
the artwork into the local HD cache. It enables HD after validation and initial
display loading succeed. The source drive can then be disconnected. No model
download or AI enhancement is needed. Switching to Classic preserves the local
copy; choosing HD again reuses it. Import/export have progress and cancellation,
and a failed or cancelled transfer preserves the previous cache.

## Appearance and rendering

HD preserves the original 640×480 logical canvas, 4:3 proportions, board layout,
sprite origins, palette identities and animation timing. The display target uses
the physical size of the fitted original canvas, including Retina scaling.
Classic retains the original 640×480 target and nearest-neighbor presentation.
Widescreen windows fill the sides with matching arena or menu scenery. The
playable board keeps its original proportions and all its cells; decorative
areas do not accept menu clicks. Portrait windows fill the top and bottom in
the same way. The last windowed content size is saved in logical screen units
and restored on the next launch; fullscreen dimensions do not replace it.
This presentation uses the active mod's existing textures and
does not require another HD preparation or enlarge the HD render target.
Scene rendering uses separate source-over alpha blending. Translucent sprite,
font and overlay edges keep an opaque background opaque, so presenting the
completed canvas does not darken its colors a second time. This display
correction reuses the prepared pages unchanged.

Sprite frames use padded canvases aligned by their original origins. Player
remaps are applied before model inference. Repeated palette dithering is resolved
while retaining its average source color. The model refines luminance; the
original palette supplies hue and saturation when prepared RGB images are
reconstructed. Nonnegative color interpolation and RGB gamut limits prevent
colored ringing. Alpha coverage is reconstructed separately from the source
mask. Tile context wraps at the source edges.
Enhanced images are repacked with filtering gutters. Font coverage is enlarged
separately, retaining the original characters and advances rather than asking
the model to invent letters. Intentional empty glyphs remain empty.

HD pages are validated and decoded by at most two background display workers.
At most one ready page is uploaded per frame, with original artwork visible
while its enhanced page loads. The GPU cache follows available RAM and the
reported GPU memory budget, between 128 MiB and 2 GiB; unknown GPU budgets use
a 1 GiB ceiling. Pages stay cached until the cache needs room. Individual pages
are limited to 4096 pixels per side;
sprite pages normally use 2048. Limits and allocation errors are checked before
the corresponding resources become active.

The restored battle HUD applies to both modes. Score and signed kill counters
fill two rows by column in original slot colors. Dead players retain counters
with red crosses. Network scores, deaths and timer use the canonical snapshot,
independently of predicted movement.
The original background, HUD and battlefield share the same 640×480 canvas.
The board starts at `(20, 68)`, without an extra field scale or gray spacer.
Scores use `FONT6.FON`, the timer uses the active `numeric font`/`infinity`
animation, and elimination markers use `xxx`. Their positions come from
`VALUELST.RES`; score colors use the RMP percentages and `COLOR.PAL` lookup.
As in the original drawing order, first-row heads can extend over the bottom
of the HUD. See [native layout verification](render-alignment.md#hud-and-window-layout).

Preparation format 2 contains the corrected color processing. HD packs from
earlier development builds need one new preparation; those old cache folders
are retained. Once the corrected pack is complete, Classic/HD switching and
import/export reuse it normally.

The original main-menu routine at `0x42baa8` selects `bomb trigger green`, not
`bomb regular green`. It advances one state per menu frame and queues a plain
blit through `0x415920`, without a player remap. The reconstruction uses the
active master animation (`TRIGANIM.ANI`) with that behavior and the original
resource-700 anchor `(332, 140 + selection × 38)`.

## Runtime performance

On the same M4 Pro, twenty-second ten-bot games with three seconds of warmup
gave these application frame timings. Both builds used the same Retina display,
physical render targets and seed; the baseline is the preserved diagnostic build
from before these HD corrections.

| HD render target | Before: FPS / 1% low | Corrected: FPS / 1% low |
| --- | --- | --- |
| 1920×1440 | 43.0 / 12.1 | 60.0 / 53.7 |
| 2803×2102 | 41.8 / 12.6 | 60.0 / 53.7 |

Corrected HD used over 99.99% enhanced draws after warmup, with no cache
evictions and no measured frames over 33 ms in that interval. Classic also
held approximately 60 FPS. These short optimized diagnostic runs measure
application frame intervals, rather than physical display/input latency or
performance on every MacBook. The larger requested window was constrained by
the screen; the table reports its actual HD target. Full results are in the
[local HD validation record](evidence/0.7.2-hd-v2-validation.json).

## Recovery

Until validation and initial GPU loading succeed, the previously saved graphics
selection stays unchanged. A failed download, unsupported GPU, helper failure,
invalid cache, missing page, allocation error or exceeded texture budget leaves
the game running in Classic with a readable reason and Retry in Graphics.

A later HD failure changes the effective mode for that session; it does not
silently rewrite a previously working saved HD preference. Startup checks a
saved cache once and falls back without an automatic retry loop. Choosing
Classic explicitly saves Classic. GPU inference never runs during a match.

## Development

The same preparation and validation pipeline is available from the asset tool:

```sh
cargo run --locked --release -p bomberman-assets -- prepare-hd \
  --assets local-assets --output build/hd-cache
cargo run --locked --release -p bomberman-assets -- validate-hd \
  --assets local-assets --output build/hd-cache/<content-key>
cargo run --locked --release -p bomberman-assets -- hd-resources
cargo run --locked --release -p bomberman-assets -- preview-hd \
  --assets local-assets --output build/hd-cache --texture FIELD1.PCX
```

`preview-hd` uses the same generation code for one original texture and writes
an inspection folder with `preview.json`. It is a developer preview, rather
than a portable HD pack. Regular preparation still includes every frame.

`prepare-hd --jobs <count>` skips calibration for reproducible measurements.
The requested count must fit the detected hardware and memory limit.

The ignored hardware test `real_parallel_enhancement_matches_serial_pages`
compares the real model's serial and parallel pages for representative original
walking colors, menu cursor, tiles and fonts. Set
`ATOMIC_BOMBERMAN_HD_BENCH_ASSETS`, `ATOMIC_BOMBERMAN_HD_BENCH_CACHE` and
`ATOMIC_BOMBERMAN_HD_BENCH_OUTPUT` to local source, helper-cache and report
directories, then run it with `cargo test -p bomberman-assets
real_parallel_enhancement_matches_serial_pages -- --ignored --nocapture`.
Native diagnostic builds can use `cargo build --profile test -p bomberman
-p bomberman-assets` to retain verification hooks with optimized image decoding.
`tools/verify_macos_hd_performance.py --game target/debug/bomberman
--hd-cache-root build/hd-cache` compares Classic and HD frame timing in identical
ten-bot games at 960×720 and 1840×1380. It records average FPS, 1% lows, frame
times and cache activity, and requires at least 98% enhanced draws after warmup.
Run it after preparation finishes so inference does not compete with rendering.
These are instrumented application timings, rather than physical display latency.
`tools/verify_macos_hud.py --game target/debug/bomberman` compares first-row
heads and last-row sprites with their source pixels; add
`--hd-cache-root build/hd-cache` to check the prepared HD sprites.
`tools/verify_macos_hd.py --mixed-lan-only` checks native HD/Classic LAN play.
Use `--mixed-relay-only --live-profile /path/to/private/connections.json` for
the same check through an existing private internet relay. Both modes also
require `--game` and `--hd-cache-root`; credentials stay in private test profiles.

Use a settings file in `build/` when testing that cache from the game. Keep
generated HD artwork, originals, tool downloads and screenshots out of source
commits and release assets. Rust dependency notices and the embedded
[HD notices](third-party-hd-notices.txt) retain the project credits.

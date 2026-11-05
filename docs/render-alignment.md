# Battle sprite alignment

Version 0.6.1 corrects the floor-flame anchor. Native `0x426d06` queries both
FRAM metadata words, then `0x426f04..0x426f46` adds X and Y before queuing the
image. The old renderer omitted X and placed Y one pixel too low. Since offsets
differ between cropped frames, differently timed explosions could show different
amounts of displacement even though color remapping preserves geometry.

For a tile-center position `(x, y)`, ground sprites use `(x, y + 17)`; floor
flames use `(x + metadata[0], y + metadata[1] - 1)`. CIMG origin subtraction is
applied after selecting that anchor. Players, bombs, shadows, Gold particles,
powerups, scenery, brick destruction and hazards now share the checked ground
anchor helper. Their previous coordinate values are preserved.

The alignment audit covers:

- The actual original floor-flame update/drawing routine, native metadata lookup,
  tile-coordinate helpers and bitmap destination rectangles: every floor-flame
  state, all ten colors, frame wrapping, and clamped destruction states in all
  eleven arenas.
- Every active battle sprite state in the imported master namespace, including
  122 sequences and 2,340 states, plus ground placement in all 165 board cells.
- 1,792 independent original player action/death/corner comparisons, including
  queued positions, palettes, carrying and pose selection.
- 1,260 GPU explosion cases and 257 original-blitter/GPU sprite comparisons,
  covering all ten remaps, clipping and 78,950,400 rendered pixels.
- 212 native hazard fixtures across 17,504 steps, including Fire God,
  regeneration, ice, teleporter/trampoline transit and extras.

Reports: [battle placement](evidence/0.6.1-battle-alignment.json),
[player poses](evidence/0.6.1-player-presentation.json),
[explosion rendering](evidence/0.6.1-explosion-renderer.json),
[sprite pixels](evidence/0.6.1-presentation-pixels.json), and
[hazard traces](evidence/0.6.1-hazards.json).

The placement check failed before the fix: the first north-tip frame at tile
`(0, 0)` had native top-left `(28, 57)`, but the renderer produced `(29, 58)`.
The corrected helper matches that case and every remaining placement fixture.

Original executable routines run in a bounded x86 emulator with the supplied
executable SHA-256 checked first. Original assets remain local and are excluded
from source and releases. The original application and platform entry points
are not started. Imported animation tables are supplied data; parser validation
is a separate boundary. GPU checks use the production renderer on macOS and do
not certify physical Windows display/audio/controller hardware.

Reproduce the placement check after building `presentation_fixture`:

```sh
python3 tools/compare_flame_presentation.py --exe /path/to/BM95.EXE \
  --assets local-assets --report build/battle-alignment.json
```

This development check needs Unicorn. The pixel comparison additionally needs
Pillow; neither dependency is required by the game or the release packages.

## HUD and window layout

The original initializer at `0x42647a` derives the board position from the screen
dimensions. At 640×480 it produces the following layout, independently matching
the original `FIELD1.PCX` artwork:

| Element | Original canvas coordinates |
| --- | --- |
| HUD background | Y=0–47 |
| Top arena wall | Y=48–67 |
| Playable board | X=20–619, Y=68–463; 15×11 cells of 40×36 |
| Bottom arena wall | Y=464–479 |
| Score columns | X=10, 110, 210, 310, 410 |
| Score rows | Y=6, 26 |
| Timer anchor | X=525, Y=36; glyph width plus 4 pixels per advance |

The previous field-only transform shrank the battlefield to 432/460 of its
original size while leaving the HUD full width. It created the narrower field
and extra gray gap visible in screenshots. The corrected renderer draws the
complete original canvas once, with the HUD before queued actors. The original
frame routine at `0x42a191..0x42a324` does not clip actors below the HUD, so
first-row heads remain visible when they overlap its lower edge.

Scores use `FONT6.FON`, four diagonal outline copies and a foreground copy at
`(+1, +1)`. Each copy clips to 90 text pixels; the native bitmap also reserves
two pixels for the outline. The timer uses
the active `numeric font` sequence from `KFONT.ANI`, or `infinity`, with CIMG
origins and no FRAM offsets. Eliminated counters use the `xxx` animation from
`MISC.ANI`. Resource positions, active sequence overrides and RMP/palette score
colors remain effective in mods.

The earlier 0.6.1 placement harness supplied Y=60 to both comparison paths. Its
reports above establish the relative flame/ground offsets, rather than the
original board origin. The current harness executes the actual initializer
before comparing placement; historical evidence is retained as recorded.

For 16:9 and wider windows, the display compositor fits the whole 4:3 canvas
and fills only the unused sides with scenery from the active arena. Menus use
their corresponding background. Portrait windows decorate the top and bottom.
The decorative rectangles stay outside the original canvas; mouse input is
mapped back through the same fitted viewport and clicks outside it are ignored.
No simulation coordinates, maps, network protocol or HD cache identity change.

The full-window comparison also checks scene opacity. Using the RGB blend
factors for alpha reduced coverage along translucent HD edges; presenting that
target over black darkened them again. The scene material now uses source-over
alpha (`source + destination × (1 − source)`), while RGB keeps its normal
textured-color blending. Miniquad's Metal backend honors the separate alpha
state too. The shaders do not recolor artwork. `compositing_fixture` checks
opaque layered rendering and identical canvas/window RGB on OpenGL; `--metal`
checks shader compilation and presentation, with readback explicitly unavailable
in the vendored Metal backend.

Reproduce the original HUD audit with the same digest-checked executable and
original files used for import:

```sh
PYTHONPATH=/path/to/unicorn python3 tools/compare_hud_layout.py \
  --exe /path/to/BM95.EXE --assets local-assets \
  --data /path/to/original/game --output build/native-hud-audit.json
```

This executes isolated native routines, not the original Windows application.
Native GPU screenshot checks use `tools/verify_macos_hud.py` and
`tools/verify_macos_widescreen.py` with a diagnostic build and private profiles.
The widescreen check compares the same-frame original canvas and full window,
checks decorative pixels and mouse input, and verifies that prepared HD pages
are retained without regeneration.

The [local validation record](evidence/2026-11-05-hud-widescreen.json) covers
Classic and HD at 4:3, 16:9, ultrawide and portrait sizes, numeric/infinity/dead
HUD fixtures, missing-cache fallback and unchanged HD cache files. It records
the tested local binaries and limits. The [0.7.4 release notes](release-notes/0.7.4.md)
describe the packaged update.

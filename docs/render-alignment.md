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

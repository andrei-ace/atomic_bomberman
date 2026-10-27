# Imported formats

The supplied `BM95.EXE` has SHA-256
`d1ff5f6fba03f20b4a161af1c75165b841e536cd9398ed2692202cee6d6b3857`.
Format conclusions below combine shipped file bytes with Ghidra/REA observations;
decompiled signatures are not treated as authoritative calling conventions.
The generated [import report](evidence/asset-import.json) records media counts
and the exact exceptions found on this disc.

| Format | Recovered representation and validation |
| --- | --- |
| MODE1/2352 | ISO9660 logical sectors are 2048 bytes at offset 16 of each 2352-byte physical sector. Also accepts ordinary 2048-byte ISO images. Directory records retain both endian lengths and offsets; readers reject disagreement, invalid extents, cycles, unsafe names, and truncated sectors. |
| PCX | 128-byte header, 8-bit indexed scanlines with PCX RLE, per-row padding, final `0x0c` palette marker and 768 RGB bytes. Bounds are checked before allocating or expanding runs. |
| `COLOR.PAL` | 768-byte VGA 6-bit palette plus 32768-byte RGB555-to-index lookup table. Palette index zero is transparent for animations. The sentinel RGB triplet at index zero is handled separately. |
| `.RMP` | 256 remapping bytes and three color percentages. Zero remap entries become identity entries, matching native loader `0x414a65`. Ten color variants preserve the supplied mapping. |
| CHFILE `.ANI` | `CHFILEANI ` signature; top-level and nested chunks have a four-byte tag, little-endian `u32` payload size, and `u16` version. Readers validate every chunk span. Native loader starts at `0x41d1a7`. |
| CIMG | Type 4 RGB555 images, 24-byte fixed header, dynamically located compressed stream (sometimes preceded by an embedded palette). Encoding zero is raw; `0x11` is RLE16. Width/height, origin, color key, and transparency flags survive conversion. Native loader `0x41c837`, origin/crop adjustment `0x41c299`. |
| RLE16 | Control `0x00..0x7f` copies `(control+1)` literal 16-bit pixels; `0x80..0xfe` repeats the following 16-bit pixel `(control&0x7f)+1` times; `0xff` ends the stream. Native decoder `0x41c0ba`. Decoded length and input bounds are enforced. |
| Animation states | Sequence state `HEAD` duration is signed; `-1` inherits the prior duration. `FRAM` contains a part count and 10-byte records (image ID and metadata). Every record and raw field is retained. Native lookup `0x41daa7` selects the first image, using the CIMG origin for placement; the other fields are not general drawing offsets. `0x41db41` exposes metadata separately, including flame alignment. Native loader `0x41cd03`. |
| `.FON` | Original bitmap glyph counts, height/spacing, per-glyph widths and data offsets are converted to a texture atlas and glyph metadata. Glyph spans and dimensions are validated. All three supplied fonts import. |
| `.RSS` | Raw signed little-endian 16-bit stereo at 22050 Hz, as specified by `SOUNDLST.RES`. WAV conversion retains sample bytes. The 25 half stereo frames receive a single zero right-channel sample; odd byte lengths fail. |
| `.SCH` | Text commands `-V`, `-N`, `-B`, `-R`, `-S`, `-P`; 15×11 terrain, brick density, ten starts, optional start team fields, and 13 powerup rules. Older schemes omit team fields. Distinct player IDs can share a start in some originals. |
| Resource tables | Ordered command/CSV records, quoted fields, duplicate numeric keys, source text, sound mappings, master animation order, and arena extra commands are preserved. Imported tables do not automatically turn every original executable tuning parameter into a runtime option. |

The unused `CLASSICS.ANI` has indexed 8-bit type 11/RLE8 imagery. It is decoded
and quantized through the global lookup for consistency with this game cache.
It is excluded by the original `MASTER.ALI`. `POWERS1.ANI` and `POWERZ.ANI`
reference a nonexistent frame 7; they are also unused by `MASTER.ALI` and are
retained as original bytes under `unused/`. An invalid required animation fails
the import.

The manifest stores source SHA-256 records separately from generated-file
checksums. PNG atlases, original indexed pixels, player variants, font atlases,
WAV files, table text, and typed metadata are generated under the selected cache
directory. Validation checks safe relative paths, required references, image
dimensions, animation/state ranges, WAV metadata, file lengths and hashes.

Import reads the source without modifying it, stages the cache in a sibling
directory, and renames only after validation. Output cannot be nested inside an
extracted source directory. A failed import does not publish a partial manifest.
Tests cover extracted directories and synthetic ISO9660/MODE1 images as well as
malformed media and cache tampering.

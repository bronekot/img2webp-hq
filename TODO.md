# TODO: fasthq tonal difference investigation

## Status: IN PROGRESS

## Original Issue
- Resize pipeline: FIXED - images were dark due to missing LinearRgb→sRGB conversion
- Non-resize pipeline: NOT FULLY FIXED - fasthq has ~1-2 RGB unit bias vs fast/normal

## Current Findings

### What was confirmed:
1. `--fast` (U8 path) produces: R=+0.0, G=+0.1, B=+0.0 vs original
2. `--fasthq` (U16 path) produces: R=+1.3, G=-0.3, B=+2.0 vs original
3. `--normal` (sharpyuv) produces: R=+0.2, G=+0.6, B=+0.5 vs original

### Root cause hypothesis (not confirmed):
Debug output shows jpegli `decode_u16` returns values like 14572 for first pixel where original JPEG has R=57.
- 14572 / 57 ≈ 255.6 (not 257)
- This suggests jpegli's U16 output is NOT simply `u8_value * 257`

### Remaining investigation needed:
1. Understand what jpegli `decode_u16` actually returns
   - Is it linear RGB? sRGB? Something else?
   - What is the exact conversion from JPEG's 8-bit DCT coefficients to 16-bit?
   
2. Compare U8 vs U16 decode paths:
   - `--fast` uses `decode()` → `working_image_from_jpeg_u8()`
   - `--fasthq` uses `decode_u16()` → `working_image_from_jpeg_u16()`
   - Are they mathematically equivalent?

3. The `import_u16()` function in simpleyuv.rs:
   - Currently: `value >> (-precision_shift(16))` = `value >> 2`
   - For value=14572, this gives 3643
   - U8 path with 57: `import_u8(57)` = `57 << 2` = 228
   - These are NOT equivalent!

## Files to investigate:
- `src/decode.rs` - jpegli decode functions
- `src/simpleyuv.rs` - `import_u16()`, `import_u8()`
- jpegli crate (fork: https://github.com/bronekot/rust-jpegli.git)

## Next steps:
1. Add debug output to compare U8 and U16 decode values for same input
2. Check jpegli documentation/source for decode_u16 specification
3. Possibly fix import_u16 or the decode path

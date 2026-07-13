# img2webp-hq

High-quality JPEG, PNG, and WebP to WebP converter. Resizing is performed in linear RGB, ICC profiles are honored, and lossy output uses gamma-aware 4:2:0 chroma subsampling by default.

## Build

```bash
cargo build --release
```

The release binary is portable: it is not compiled for the build machine's CPU. SIMD in the image resizer and worker threads are selected at runtime.

## Usage

```bash
target/release/img2webp-hq input.jpg -o output.webp --max-side 1600 -q 82
```

Useful modes:

- `--fast` uses the 8-bit decode path for maximum throughput.
- `--fasthq` keeps the high-bit-depth path and parallelizes the ICC transforms used by lossy high-quality resizing. It cannot be combined with `--fast`, `--sharpyuv`, lossless, or near-lossless modes.
- `--sharpyuv` uses libwebp SharpYUV instead of the default SimpleYUV path.
- `--lossless` and `--near_lossless N` enable the corresponding WebP modes.
- `--metadata none|all|exif|icc|xmp` controls preserved metadata; the default is `icc`.
- `--width`, `--height`, `--max-width`, `--max-height`, and `--max-side` control resize dimensions. `--no-upscale` prevents enlargement.

Run `img2webp-hq --help` for the complete option list.

## Quality and regression checks

The normal test suite verifies all encoding modes, ICC handling, orientation, RGB/RGBA and 8/16-bit paths, and exact YUV equivalence of the accelerated `--fasthq` resize path:

```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

The explicit release-mode quality gate compares default and `--fasthq` output against a lossless resized reference using RGB PSNR and luma DSSIM, and rejects either a quality regression or a larger file:

```bash
cargo test --release fasthq_resize_quality_and_size_gate -- --ignored --nocapture
```

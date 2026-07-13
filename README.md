# img2webp-hq

High-quality JPEG, PNG, and WebP to WebP converter and Rust library. Resizing is performed in linear RGB, ICC profiles are honored, and lossy output uses gamma-aware 4:2:0 chroma subsampling by default.

The package provides both an in-memory library API and the `img2webp-hq` command-line tool. They use the same conversion pipeline and options.

## Command-line tool

Build the release binary:

```bash
cargo build --release
```

Or install it directly from GitHub:

```bash
cargo install --git https://github.com/bronekot/img2webp-hq.git --locked
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

## Rust library

Add the project as a dependency from GitHub:

```toml
[dependencies]
img2webp-hq = { git = "https://github.com/bronekot/img2webp-hq.git", default-features = false }
```

Use `convert` when the encoded input image is already in memory:

```rust
use img2webp_hq::{ConversionOptions, MetadataPolicy, convert};

let source = std::fs::read("input.jpg")?;
let options = ConversionOptions::default()
    .with_quality(82.0)
    .with_max_side(1600)
    .with_metadata(MetadataPolicy::None);
let webp = convert(&source, &options)?;
std::fs::write("output.webp", webp)?;
# Ok::<(), img2webp_hq::Error>(())
```

For file-to-file conversion, use the convenience adapter:

```rust
use img2webp_hq::{ConversionOptions, convert_file};

let options = ConversionOptions::default()
    .with_quality(82.0)
    .with_max_side(1600);
convert_file("input.jpg", "output.webp", &options)?;
# Ok::<(), img2webp_hq::Error>(())
```

The default `cli` feature builds the command-line tool and enables `clap`. Library-only consumers can disable default features as shown above to avoid the CLI dependency.

## Quality and regression checks

The normal test suite verifies all encoding modes, ICC handling, orientation, RGB/RGBA and 8/16-bit paths, and exact YUV equivalence of the accelerated `--fasthq` resize path:

```bash
cargo test --all-targets
cargo test --no-default-features --test public_api
cargo clippy --all-targets -- -D warnings
```

The explicit release-mode quality gate compares default and `--fasthq` output against a lossless resized reference using RGB PSNR and luma DSSIM, and rejects either a quality regression or a larger file:

```bash
cargo test --release fasthq_resize_quality_and_size_gate -- --ignored --nocapture
```

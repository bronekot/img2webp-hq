//! High-quality JPEG, PNG, and WebP to WebP conversion.
//!
//! Use [`convert`] for in-memory conversion or [`convert_file`] for a
//! filesystem-based workflow. Both APIs use the same processing pipeline as
//! the `img2webp-hq` command-line tool.

#[cfg(feature = "cli")]
mod cli;
mod cms;
mod config;
mod decode;
mod encode;
mod error;
mod metadata;
mod pipeline;
mod resize;
mod sharpyuv;
mod simpleyuv;

#[cfg(test)]
mod integration_tests;

use std::fs;
use std::path::Path;

pub use config::{ConversionOptions, Mode, ResizeFilter, ResizeOptions};
pub use error::{Error, Result};
pub use metadata::MetadataPolicy;

/// Converts an encoded JPEG, PNG, or WebP image to WebP in memory.
pub fn convert(input: &[u8], options: &ConversionOptions) -> Result<Vec<u8>> {
    pipeline::convert(input, options)
}

/// Converts an image file to WebP using the same pipeline as [`convert`].
pub fn convert_file(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: &ConversionOptions,
) -> Result<()> {
    let input = fs::read(input)?;
    let webp = convert(&input, options)?;
    fs::write(output, webp)?;
    Ok(())
}

/// Parses command-line arguments and performs one file conversion.
#[cfg(feature = "cli")]
pub fn run_from_env() -> Result<()> {
    let job = cli::parse_from_env()?;
    convert_file(job.input, job.output, &job.options)
}

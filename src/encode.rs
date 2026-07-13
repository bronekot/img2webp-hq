use rayon::prelude::*;
use webpx::{Encoder, EncoderConfig, Unstoppable, YuvPlanesRef, embed_exif, embed_icc, embed_xmp};

use crate::cli::{Job, Mode};
use crate::cms::ColorPipeline;
use crate::decode::{DecodedInput, WorkingColorSpace, WorkingData, WorkingImage};
use crate::error::{Error, Result};
use crate::sharpyuv;
use crate::simpleyuv;

const PARALLEL_MIN_PIXELS: usize = 256 * 1024;

pub fn encode(
    job: &Job,
    decoded: &DecodedInput,
    color_pipeline: Option<&ColorPipeline>,
) -> Result<Vec<u8>> {
    match job.mode {
        Mode::Lossy => encode_lossy(job, decoded, color_pipeline),
        Mode::Lossless => encode_lossless(job, decoded, None),
        Mode::NearLossless(value) => encode_lossless(job, decoded, Some(value)),
    }
}

fn encode_lossy(
    job: &Job,
    decoded: &DecodedInput,
    color_pipeline: Option<&ColorPipeline>,
) -> Result<Vec<u8>> {
    let planes = if job.uses_simpleyuv() {
        let color_pipeline = color_pipeline.expect("SimpleYUV always creates a color pipeline");
        simpleyuv::working_image_to_yuv420(&decoded.image, color_pipeline)?
    } else {
        let assume_linear = decoded.image.color_space == WorkingColorSpace::LinearRgb;
        sharpyuv::working_image_to_yuv420(&decoded.image, assume_linear)?
    };

    let config = base_config(job)
        .quality(job.quality)
        .method(job.method)
        .alpha_quality(job.alpha_quality)
        .sharp_yuv(!job.uses_simpleyuv())
        .exact(job.exact);

    let webp = Encoder::new_yuv(YuvPlanesRef::from(&planes))
        .config(config)
        .encode(Unstoppable)
        .map_err(|err| Error::encode(format!("lossy WebP encode failed: {err}")))?;

    attach_metadata(webp, job, decoded)
}

fn encode_lossless(
    job: &Job,
    decoded: &DecodedInput,
    near_lossless: Option<u8>,
) -> Result<Vec<u8>> {
    let argb = working_image_to_argb8(&decoded.image);
    let mut config = base_config(job)
        .quality(job.quality)
        .method(job.method)
        .lossless(true)
        .alpha_quality(job.alpha_quality)
        .exact(job.exact);

    if let Some(value) = near_lossless {
        config = config.near_lossless(value);
    }

    let webp = Encoder::new_argb(&argb, decoded.image.width, decoded.image.height)
        .config(config)
        .encode(Unstoppable)
        .map_err(|err| Error::encode(format!("lossless WebP encode failed: {err}")))?;

    attach_metadata(webp, job, decoded)
}

fn base_config(job: &Job) -> EncoderConfig {
    let mut config = EncoderConfig::new().thread_level(1);
    if let Some(value) = job.sns_strength {
        config = config.sns_strength(value);
    }
    if let Some(value) = job.filter_strength {
        config = config.filter_strength(value);
    }
    config
}

fn attach_metadata(mut webp: Vec<u8>, job: &Job, decoded: &DecodedInput) -> Result<Vec<u8>> {
    if job.metadata.keep_icc()
        && let Some(icc) = &decoded.metadata.icc
    {
        webp = embed_icc(&webp, icc)
            .map_err(|err| Error::encode(format!("failed to attach ICC profile: {err}")))?;
    }
    if job.metadata.keep_exif()
        && let Some(exif) = &decoded.metadata.exif
    {
        webp = embed_exif(&webp, exif)
            .map_err(|err| Error::encode(format!("failed to attach EXIF metadata: {err}")))?;
    }
    if job.metadata.keep_xmp()
        && let Some(xmp) = &decoded.metadata.xmp
    {
        webp = embed_xmp(&webp, xmp)
            .map_err(|err| Error::encode(format!("failed to attach XMP metadata: {err}")))?;
    }

    Ok(webp)
}

fn working_image_to_argb8(image: &WorkingImage) -> Vec<u32> {
    let parallel =
        (image.width as usize).saturating_mul(image.height as usize) >= PARALLEL_MIN_PIXELS;
    match &image.data {
        WorkingData::Rgb8(data) if parallel => data.par_chunks_exact(3).map(pack_rgb8).collect(),
        WorkingData::Rgba8(data) if parallel => data.par_chunks_exact(4).map(pack_rgba8).collect(),
        WorkingData::Rgb16(data) if parallel => data.par_chunks_exact(3).map(pack_rgb16).collect(),
        WorkingData::Rgba16(data) if parallel => {
            data.par_chunks_exact(4).map(pack_rgba16).collect()
        }
        WorkingData::Rgb8(data) => data.chunks_exact(3).map(pack_rgb8).collect(),
        WorkingData::Rgba8(data) => data.chunks_exact(4).map(pack_rgba8).collect(),
        WorkingData::Rgb16(data) => data.chunks_exact(3).map(pack_rgb16).collect(),
        WorkingData::Rgba16(data) => data.chunks_exact(4).map(pack_rgba16).collect(),
    }
}

fn pack_rgb8(pixel: &[u8]) -> u32 {
    (u8::MAX as u32) << 24 | ((pixel[0] as u32) << 16) | ((pixel[1] as u32) << 8) | pixel[2] as u32
}

fn pack_rgba8(pixel: &[u8]) -> u32 {
    ((pixel[3] as u32) << 24)
        | ((pixel[0] as u32) << 16)
        | ((pixel[1] as u32) << 8)
        | pixel[2] as u32
}

fn pack_rgb16(pixel: &[u16]) -> u32 {
    (u8::MAX as u32) << 24
        | ((down16_to_8(pixel[0]) as u32) << 16)
        | ((down16_to_8(pixel[1]) as u32) << 8)
        | down16_to_8(pixel[2]) as u32
}

fn pack_rgba16(pixel: &[u16]) -> u32 {
    ((down16_to_8(pixel[3]) as u32) << 24)
        | ((down16_to_8(pixel[0]) as u32) << 16)
        | ((down16_to_8(pixel[1]) as u32) << 8)
        | down16_to_8(pixel[2]) as u32
}

fn down16_to_8(value: u16) -> u8 {
    ((value as u32 * 255 + 32767) / 65535) as u8
}

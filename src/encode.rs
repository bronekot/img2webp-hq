use webpx::{Encoder, EncoderConfig, Unstoppable, YuvPlanesRef, embed_exif, embed_icc, embed_xmp};

use crate::cli::{Job, Mode};
use crate::cms::ResizeColorPipeline;
use crate::decode::{DecodedInput, WorkingColorSpace, WorkingData, WorkingImage};
use crate::error::{Error, Result};
use crate::sharpyuv;
use crate::simpleyuv;

pub fn encode(job: &Job, decoded: &DecodedInput) -> Result<Vec<u8>> {
    match job.mode {
        Mode::Lossy => encode_lossy(job, decoded),
        Mode::Lossless => encode_lossless(job, decoded, None),
        Mode::NearLossless(value) => encode_lossless(job, decoded, Some(value)),
    }
}

fn encode_lossy(job: &Job, decoded: &DecodedInput) -> Result<Vec<u8>> {
    let planes = if job.uses_simpleyuv() {
        let cms = ResizeColorPipeline::new(decoded.metadata.icc.as_deref())?;
        if job.fast_hq {
            if let Some(planes) = simpleyuv::rgba_to_yuv420_sharpish_linear_u16(&decoded.image) {
                planes?
            } else {
                simpleyuv::rgba_to_yuv420(&decoded.image, &cms)?
            }
        } else {
            simpleyuv::rgba_to_yuv420(&decoded.image, &cms)?
        }
    } else {
        let assume_linear = decoded.image.color_space == WorkingColorSpace::LinearRgb;
        match &decoded.image.data {
            WorkingData::U8(data) => sharpyuv::rgba8_to_yuv420(
                data,
                decoded.image.width,
                decoded.image.height,
                assume_linear,
            )?,
            WorkingData::U16(data) => sharpyuv::rgba16_to_yuv420(
                data,
                decoded.image.width,
                decoded.image.height,
                assume_linear,
            )?,
        }
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
    let argb = rgba_to_argb8(&decoded.image);
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
    let mut config = EncoderConfig::new();
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

fn rgba_to_argb8(image: &WorkingImage) -> Vec<u32> {
    match &image.data {
        WorkingData::U8(data) => data
            .chunks_exact(4)
            .map(|pixel| {
                let r = pixel[0] as u32;
                let g = pixel[1] as u32;
                let b = pixel[2] as u32;
                let a = pixel[3] as u32;
                (a << 24) | (r << 16) | (g << 8) | b
            })
            .collect(),
        WorkingData::U16(data) => data
            .chunks_exact(4)
            .map(|pixel| {
                let r = down16_to_8(pixel[0]) as u32;
                let g = down16_to_8(pixel[1]) as u32;
                let b = down16_to_8(pixel[2]) as u32;
                let a = down16_to_8(pixel[3]) as u32;
                (a << 24) | (r << 16) | (g << 8) | b
            })
            .collect(),
    }
}

fn down16_to_8(value: u16) -> u8 {
    ((value as u32 * 255 + 32767) / 65535) as u8
}

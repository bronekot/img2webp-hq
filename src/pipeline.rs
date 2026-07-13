use std::fs;

use crate::cli::{Job, Mode};
use crate::cms::{self, ColorPipeline};
use crate::decode;
use crate::encode;
use crate::error::Result;
use crate::resize;

pub fn run(job: Job) -> Result<()> {
    job.validate()?;
    let mut decoded = decode::decode(&job.input, job.uses_fast_decode())?;

    let target =
        resize::compute_target_size(decoded.image.width, decoded.image.height, &job.resize)?;
    let fast_hq_resize = job.fast_hq && target.resized;
    let needs_linear_pipeline =
        target.resized || (job.uses_simpleyuv() && matches!(job.mode, Mode::Lossy));
    let color_pipeline = if needs_linear_pipeline {
        Some(ColorPipeline::new(decoded.metadata.icc.as_deref())?)
    } else {
        cms::validate_source_profile(decoded.metadata.icc.as_deref(), false)?;
        None
    };

    if target.resized {
        let color_pipeline = color_pipeline
            .as_ref()
            .expect("resizing always creates a color pipeline");
        if fast_hq_resize {
            color_pipeline.to_linear_parallel_in_place(&mut decoded.image)?;
        } else {
            color_pipeline.to_linear_in_place(&mut decoded.image)?;
        }
        let filter = resize::resolve_filter(
            decoded.image.width,
            decoded.image.height,
            target,
            job.resize.filter,
        );
        decoded.image = resize::resize(decoded.image, target, filter)?;
        if fast_hq_resize {
            color_pipeline.convert_from_linear_parallel_in_place(&mut decoded.image)?;
        } else {
            color_pipeline.convert_from_linear_in_place(&mut decoded.image)?;
        }
    }

    let webp = encode::encode(&job, &decoded, color_pipeline.as_ref())?;
    fs::write(&job.output, webp)?;
    Ok(())
}

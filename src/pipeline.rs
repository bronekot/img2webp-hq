use std::fs;

use crate::cli::{Job, Mode};
use crate::cms::{self, ResizeColorPipeline};
use crate::decode;
use crate::encode;
use crate::error::Result;
use crate::resize;

pub fn run(job: Job) -> Result<()> {
    let mut decoded = decode::decode(&job.input, job.uses_fast_decode())?;

    let target =
        resize::compute_target_size(decoded.image.width, decoded.image.height, &job.resize)?;
    let needs_linear_pipeline =
        target.resized || (job.uses_simpleyuv() && matches!(job.mode, Mode::Lossy));
    cms::validate_source_profile(decoded.metadata.icc.as_deref(), needs_linear_pipeline)?;
    if target.resized {
        let cms = ResizeColorPipeline::new(decoded.metadata.icc.as_deref())?;
        cms.to_linear_in_place(&mut decoded.image)?;
        let filter = resize::resolve_filter(
            decoded.image.width,
            decoded.image.height,
            target,
            job.resize.filter,
        );
        decoded.image = resize::resize(decoded.image, target, filter)?;
        if !matches!(job.mode, Mode::Lossy) {
            cms.from_linear_in_place(&mut decoded.image)?;
        }
    }

    let webp = encode::encode(&job, &decoded)?;
    fs::write(&job.output, webp)?;
    Ok(())
}

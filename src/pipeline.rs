use std::fs;

use crate::cli::Job;
use crate::cms::{self, ResizeColorPipeline};
use crate::decode;
use crate::encode;
use crate::error::Result;
use crate::resize;

pub fn run(job: Job) -> Result<()> {
    let mut decoded = decode::decode(&job.input)?;

    let target =
        resize::compute_target_size(decoded.image.width, decoded.image.height, &job.resize)?;
    cms::validate_source_profile(decoded.metadata.icc.as_deref(), target.resized)?;
    if target.resized {
        let cms = ResizeColorPipeline::new(decoded.metadata.icc.as_deref())?;
        cms.to_linear_in_place(&mut decoded.image.data)?;
        let filter = resize::resolve_filter(
            decoded.image.width,
            decoded.image.height,
            target,
            job.resize.filter,
        );
        decoded.image = resize::resize_rgba16(decoded.image, target, filter)?;
        cms.from_linear_in_place(&mut decoded.image.data)?;
    }

    let webp = encode::encode(&job, &decoded)?;
    fs::write(&job.output, webp)?;
    Ok(())
}

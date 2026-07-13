use crate::cms::{self, ColorPipeline};
use crate::config::{ConversionOptions, Mode};
use crate::decode;
use crate::encode;
use crate::error::Result;
use crate::resize;

pub fn convert(input: &[u8], options: &ConversionOptions) -> Result<Vec<u8>> {
    options.validate()?;
    let mut decoded = decode::decode(input, options.uses_fast_decode())?;

    let target =
        resize::compute_target_size(decoded.image.width, decoded.image.height, &options.resize)?;
    let fast_hq_resize = options.fast_hq && target.resized;
    let needs_linear_pipeline =
        target.resized || (options.uses_simpleyuv() && matches!(options.mode, Mode::Lossy));
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
            options.resize.filter,
        );
        decoded.image = resize::resize(decoded.image, target, filter)?;
        if fast_hq_resize {
            color_pipeline.convert_from_linear_parallel_in_place(&mut decoded.image)?;
        } else {
            color_pipeline.convert_from_linear_in_place(&mut decoded.image)?;
        }
    }

    encode::encode(options, &decoded, color_pipeline.as_ref())
}

use fast_image_resize::images::Image;
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions as FirResizeOptions, Resizer,
};

use crate::cli::{ResizeFilter, ResizeOptions};
use crate::decode::{WorkingData, WorkingImage};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub struct TargetSize {
    pub width: u32,
    pub height: u32,
    pub resized: bool,
}

pub fn resolve_filter(
    src_width: u32,
    src_height: u32,
    target: TargetSize,
    requested: Option<ResizeFilter>,
) -> ResizeFilter {
    if let Some(filter) = requested {
        return filter;
    }

    if target.width > src_width || target.height > src_height {
        ResizeFilter::CatmullRom
    } else {
        ResizeFilter::Lanczos3
    }
}

pub fn compute_target_size(
    src_width: u32,
    src_height: u32,
    options: &ResizeOptions,
) -> Result<TargetSize> {
    let (mut width, mut height) =
        if let (Some(width), Some(height)) = (options.width, options.height) {
            (width, height)
        } else if let Some(width) = options.width {
            let height = scale_rounded(src_height, width, src_width);
            (width, height)
        } else if let Some(height) = options.height {
            let width = scale_rounded(src_width, height, src_height);
            (width, height)
        } else if let Some(side) = options.max_side {
            fit_inside(src_width, src_height, side, side)
        } else if options.max_width.is_some() || options.max_height.is_some() {
            fit_inside(
                src_width,
                src_height,
                options.max_width.unwrap_or(src_width),
                options.max_height.unwrap_or(src_height),
            )
        } else {
            (src_width, src_height)
        };

    if width == 0 || height == 0 {
        return Err(Error::invalid("calculated target size is zero"));
    }

    if options.no_upscale {
        width = width.min(src_width);
        height = height.min(src_height);
    }

    if width > 16_383 || height > 16_383 {
        return Err(Error::invalid(
            "resulting WebP dimensions exceed 16383x16383",
        ));
    }

    Ok(TargetSize {
        width,
        height,
        resized: width != src_width || height != src_height,
    })
}

pub fn resize(image: WorkingImage, target: TargetSize, filter: ResizeFilter) -> Result<WorkingImage> {
    if !target.resized {
        return Ok(image);
    }

    let filter = match filter {
        ResizeFilter::Lanczos3 => FilterType::Lanczos3,
        ResizeFilter::CatmullRom => FilterType::CatmullRom,
        ResizeFilter::Mitchell => FilterType::Mitchell,
    };
    let options = FirResizeOptions::new()
        .resize_alg(ResizeAlg::Convolution(filter))
        .use_alpha(true);

    match image.data {
        WorkingData::U8(data) => {
            let src = Image::from_vec_u8(image.width, image.height, data, PixelType::U8x4)
                .map_err(|err| Error::invalid(format!("invalid source buffer for resize: {err}")))?;
            let mut dst = Image::new(target.width, target.height, PixelType::U8x4);

            let mut resizer = Resizer::new();
            resizer
                .resize(&src, &mut dst, Some(&options))
                .map_err(|err| Error::encode(format!("resize failed: {err}")))?;

            Ok(WorkingImage {
                width: target.width,
                height: target.height,
                data: WorkingData::U8(dst.into_vec()),
            })
        }
        WorkingData::U16(data) => {
            let src = Image::from_vec_u8(
                image.width,
                image.height,
                u16_to_bytes(data),
                PixelType::U16x4,
            )
            .map_err(|err| Error::invalid(format!("invalid source buffer for resize: {err}")))?;
            let mut dst = Image::new(target.width, target.height, PixelType::U16x4);

            let mut resizer = Resizer::new();
            resizer
                .resize(&src, &mut dst, Some(&options))
                .map_err(|err| Error::encode(format!("resize failed: {err}")))?;

            Ok(WorkingImage {
                width: target.width,
                height: target.height,
                data: WorkingData::U16(bytes_to_u16(dst.into_vec())),
            })
        }
    }
}

fn scale_rounded(value: u32, numer: u32, denom: u32) -> u32 {
    ((value as u64 * numer as u64) / denom as u64) as u32
}

fn fit_inside(src_width: u32, src_height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if src_width <= max_width && src_height <= max_height {
        return (src_width, src_height);
    }

    let scale_w = max_width as f64 / src_width as f64;
    let scale_h = max_height as f64 / src_height as f64;
    let scale = scale_w.min(scale_h);

    let width = ((src_width as f64) * scale).floor().max(1.0) as u32;
    let height = ((src_height as f64) * scale).floor().max(1.0) as u32;
    (width, height)
}

fn bytes_to_u16(bytes: Vec<u8>) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_ne_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn u16_to_bytes(values: Vec<u16>) -> Vec<u8> {
    values.into_iter().flat_map(u16::to_ne_bytes).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_preserves_aspect_ratio() {
        let result = compute_target_size(
            4000,
            2000,
            &ResizeOptions {
                width: None,
                height: None,
                max_width: Some(1000),
                max_height: Some(1000),
                max_side: None,
                no_upscale: false,
                filter: Some(ResizeFilter::Lanczos3),
            },
        )
        .unwrap();
        assert_eq!((result.width, result.height), (1000, 500));
    }

    #[test]
    fn max_side_scales_landscape_by_longest_edge() {
        let result = compute_target_size(
            4000,
            2000,
            &ResizeOptions {
                width: None,
                height: None,
                max_width: None,
                max_height: None,
                max_side: Some(1200),
                no_upscale: false,
                filter: None,
            },
        )
        .unwrap();

        assert_eq!((result.width, result.height), (1200, 600));
    }

    #[test]
    fn max_side_scales_portrait_by_longest_edge() {
        let result = compute_target_size(
            2000,
            4000,
            &ResizeOptions {
                width: None,
                height: None,
                max_width: None,
                max_height: None,
                max_side: Some(1200),
                no_upscale: false,
                filter: None,
            },
        )
        .unwrap();

        assert_eq!((result.width, result.height), (600, 1200));
    }

    #[test]
    fn auto_filter_uses_catmullrom_for_upscale() {
        let target = TargetSize {
            width: 800,
            height: 800,
            resized: true,
        };

        assert_eq!(
            resolve_filter(400, 400, target, None),
            ResizeFilter::CatmullRom
        );
    }

    #[test]
    fn auto_filter_uses_lanczos3_for_downscale() {
        let target = TargetSize {
            width: 400,
            height: 400,
            resized: true,
        };

        assert_eq!(
            resolve_filter(800, 800, target, None),
            ResizeFilter::Lanczos3
        );
    }
}

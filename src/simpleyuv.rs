use rayon::prelude::*;
use webpx::YuvPlanes;

#[cfg(test)]
use crate::cms::LinearU16Tables;
use crate::cms::{ColorPipeline, SimpleYuvTables};
use crate::decode::{WorkingColorSpace, WorkingData, WorkingImage};
use crate::error::Result;

const YUV_FIX: i32 = 16;
#[cfg(test)]
#[allow(dead_code)]
const YUV_HALF: i64 = 1 << (YUV_FIX - 1);
const WEBP_RGB_TO_Y: [i32; 4] = [16839, 33059, 6420, 16 << 16];
const WEBP_RGB_TO_U: [i32; 4] = [-9719, -19081, 28800, 128 << 16];
const WEBP_RGB_TO_V: [i32; 4] = [28800, -24116, -4684, 128 << 16];
const PARALLEL_MIN_PIXELS: usize = 256 * 1024;

#[derive(Clone, Copy)]
struct SampleLayout {
    width: u32,
    height: u32,
    channels: usize,
    color_space: WorkingColorSpace,
}

pub fn working_image_to_yuv420(image: &WorkingImage, cms: &ColorPipeline) -> Result<YuvPlanes> {
    let tables = cms.simpleyuv_tables()?;
    match &image.data {
        WorkingData::Rgb8(data) => samples8_to_yuv420(
            data,
            image.width,
            image.height,
            image.color_space,
            &tables,
            3,
            false,
        ),
        WorkingData::Rgba8(data) => samples8_to_yuv420(
            data,
            image.width,
            image.height,
            image.color_space,
            &tables,
            4,
            true,
        ),
        WorkingData::Rgb16(data) => samples16_to_yuv420(
            data,
            image.width,
            image.height,
            image.color_space,
            &tables,
            3,
            false,
        ),
        WorkingData::Rgba16(data) => samples16_to_yuv420(
            data,
            image.width,
            image.height,
            image.color_space,
            &tables,
            4,
            true,
        ),
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub fn rgba_to_yuv420_sharpish_linear_u16(image: &WorkingImage) -> Option<Result<YuvPlanes>> {
    if image.color_space != WorkingColorSpace::LinearRgb {
        return None;
    }

    let (data, channels, with_alpha) = match &image.data {
        WorkingData::Rgb16(data) => (data.as_slice(), 3, false),
        WorkingData::Rgba16(data) => (data.as_slice(), 4, true),
        WorkingData::Rgb8(_) | WorkingData::Rgba8(_) => return None,
    };

    Some(rgba16_to_yuv420_sharpish_linear(
        data,
        image.width,
        image.height,
        channels,
        with_alpha,
    ))
}

fn effective_u16_bit_depth() -> u8 {
    (16i32 + precision_shift(16)) as u8
}

#[cfg(test)]
#[allow(dead_code)]
fn rgba16_to_yuv420_sharpish_linear(
    image: &[u16],
    width: u32,
    height: u32,
    channels: usize,
    with_alpha: bool,
) -> Result<YuvPlanes> {
    let mut planes = YuvPlanes::new(width, height, with_alpha);
    let w = width as usize;
    let h = height as usize;
    let uv_width = width.div_ceil(2) as usize;
    let bit_depth = effective_u16_bit_depth();
    let sfix = precision_shift(bit_depth);
    let y_coeffs = scale_matrix(&WEBP_RGB_TO_Y, bit_depth);
    let u_coeffs = scale_matrix(&WEBP_RGB_TO_U, bit_depth);
    let v_coeffs = scale_matrix(&WEBP_RGB_TO_V, bit_depth);

    let mut luma = vec![0u16; w * h];
    let mut residuals = vec![[0i32; 3]; uv_width * height.div_ceil(2) as usize];

    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) * channels;
            let r = import_u16(image[idx]) as i32;
            let g = import_u16(image[idx + 1]) as i32;
            let b = import_u16(image[idx + 2]) as i32;
            luma[y * w + x] = rgb_to_gray(r, g, b);
        }
    }

    for block_y in (0..height).step_by(2) {
        for block_x in (0..width).step_by(2) {
            let mut sum_r = 0u32;
            let mut sum_g = 0u32;
            let mut sum_b = 0u32;

            for dy in 0..2 {
                for dx in 0..2 {
                    let (px, py) = block_pixel(width, height, block_x, block_y, dx, dy);
                    let idx = ((py * width + px) as usize) * channels;
                    sum_r += import_u16(image[idx]) as u32;
                    sum_g += import_u16(image[idx + 1]) as u32;
                    sum_b += import_u16(image[idx + 2]) as u32;
                }
            }

            let avg_r = average_linear(sum_r) as i32;
            let avg_g = average_linear(sum_g) as i32;
            let avg_b = average_linear(sum_b) as i32;
            let avg_w = rgb_to_gray(avg_r, avg_g, avg_b) as i32;
            let residual = [avg_r - avg_w, avg_g - avg_w, avg_b - avg_w];
            let uv_idx = ((block_y / 2) as usize) * uv_width + (block_x / 2) as usize;
            residuals[uv_idx] = residual;
            planes.u[uv_idx] =
                rgb_to_component_8bit(residual[0], residual[1], residual[2], &u_coeffs, sfix);
            planes.v[uv_idx] =
                rgb_to_component_8bit(residual[0], residual[1], residual[2], &v_coeffs, sfix);
        }
    }

    for y in 0..h {
        for x in 0..w {
            let uv_idx = (y / 2) * uv_width + (x / 2);
            let residual = residuals[uv_idx];
            let base = luma[y * w + x] as i32;
            planes.y[y * w + x] = rgb_to_component_8bit(
                base + residual[0],
                base + residual[1],
                base + residual[2],
                &y_coeffs,
                sfix,
            );
        }
    }

    if let Some(alpha) = &mut planes.a {
        for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(channels)) {
            *dst = down16_to_8(pixel[channels - 1]);
        }
    }

    Ok(planes)
}

#[cfg(test)]
pub fn linear_u16_to_yuv420(image: &WorkingImage, tables: &LinearU16Tables) -> Result<YuvPlanes> {
    if image.color_space != WorkingColorSpace::LinearRgb {
        return Err(crate::error::Error::color(
            "--fasthq expected linear RGB input",
        ));
    }

    let (data, channels, with_alpha) = match &image.data {
        WorkingData::Rgb16(data) => (data.as_slice(), 3, false),
        WorkingData::Rgba16(data) => (data.as_slice(), 4, true),
        WorkingData::Rgb8(_) | WorkingData::Rgba8(_) => {
            return Err(crate::error::Error::color(
                "--fasthq requires 16-bit working data",
            ));
        }
    };

    linear_samples_u16_to_yuv420(
        data,
        image.width,
        image.height,
        channels,
        with_alpha,
        tables,
    )
}

#[cfg(test)]
fn linear_samples_u16_to_yuv420(
    image: &[u16],
    width: u32,
    height: u32,
    channels: usize,
    with_alpha: bool,
    tables: &LinearU16Tables,
) -> Result<YuvPlanes> {
    let mut planes = YuvPlanes::new(width, height, with_alpha);
    let uv_width = width.div_ceil(2) as usize;
    let bit_depth = effective_u16_bit_depth();
    let sfix = precision_shift(bit_depth);
    let y_coeffs = scale_matrix(&WEBP_RGB_TO_Y, bit_depth);
    let u_coeffs = scale_matrix(&WEBP_RGB_TO_U, bit_depth);
    let v_coeffs = scale_matrix(&WEBP_RGB_TO_V, bit_depth);

    planes
        .y
        .par_iter_mut()
        .zip(image.par_chunks_exact(channels))
        .for_each(|(dst, pixel)| {
            let r = import_u16(tables.linear_to_source[0][pixel[0] as usize]) as i32;
            let g = import_u16(tables.linear_to_source[1][pixel[1] as usize]) as i32;
            let b = import_u16(tables.linear_to_source[2][pixel[2] as usize]) as i32;
            *dst = rgb_to_component_8bit(r, g, b, &y_coeffs, sfix);
        });

    planes
        .u
        .par_iter_mut()
        .zip(planes.v.par_iter_mut())
        .enumerate()
        .for_each(|(uv_idx, (u, v))| {
            let block_x = ((uv_idx % uv_width) * 2) as u32;
            let block_y = ((uv_idx / uv_width) * 2) as u32;
            let mut sums = [0u32; 3];
            for dy in 0..2 {
                for dx in 0..2 {
                    let (px, py) = block_pixel(width, height, block_x, block_y, dx, dy);
                    let idx = ((py * width + px) as usize) * channels;
                    let source_r = tables.linear_to_source[0][image[idx] as usize];
                    let source_g = tables.linear_to_source[1][image[idx + 1] as usize];
                    let source_b = tables.linear_to_source[2][image[idx + 2] as usize];
                    sums[0] +=
                        tables.chroma_source_to_linear[0][import_u16(source_r) as usize] as u32;
                    sums[1] +=
                        tables.chroma_source_to_linear[1][import_u16(source_g) as usize] as u32;
                    sums[2] +=
                        tables.chroma_source_to_linear[2][import_u16(source_b) as usize] as u32;
                }
            }
            let rgb = [
                tables.chroma_linear_to_source[0][average_linear(sums[0]) as usize],
                tables.chroma_linear_to_source[1][average_linear(sums[1]) as usize],
                tables.chroma_linear_to_source[2][average_linear(sums[2]) as usize],
            ];
            *u =
                rgb_to_component_8bit(rgb[0] as i32, rgb[1] as i32, rgb[2] as i32, &u_coeffs, sfix);
            *v =
                rgb_to_component_8bit(rgb[0] as i32, rgb[1] as i32, rgb[2] as i32, &v_coeffs, sfix);
        });

    if let Some(alpha) = &mut planes.a {
        alpha
            .par_iter_mut()
            .zip(image.par_chunks_exact(channels))
            .for_each(|(dst, pixel)| *dst = down16_to_8(pixel[channels - 1]));
    }

    Ok(planes)
}

fn samples8_to_yuv420(
    image: &[u8],
    width: u32,
    height: u32,
    color_space: WorkingColorSpace,
    tables: &SimpleYuvTables,
    channels: usize,
    with_alpha: bool,
) -> Result<YuvPlanes> {
    let mut planes = YuvPlanes::new(width, height, with_alpha);
    let layout = SampleLayout {
        width,
        height,
        channels,
        color_space,
    };

    fill_y_plane_u8(image, channels, &mut planes.y);
    fill_uv_plane_u8(image, layout, tables, &mut planes.u, &mut planes.v);

    if let Some(alpha) = &mut planes.a {
        if alpha.len() >= PARALLEL_MIN_PIXELS {
            alpha
                .par_iter_mut()
                .zip(image.par_chunks_exact(channels))
                .for_each(|(dst, pixel)| *dst = pixel[channels - 1]);
        } else {
            for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(channels)) {
                *dst = pixel[channels - 1];
            }
        }
    }

    Ok(planes)
}

fn samples16_to_yuv420(
    image: &[u16],
    width: u32,
    height: u32,
    color_space: WorkingColorSpace,
    tables: &SimpleYuvTables,
    channels: usize,
    with_alpha: bool,
) -> Result<YuvPlanes> {
    let mut planes = YuvPlanes::new(width, height, with_alpha);
    let layout = SampleLayout {
        width,
        height,
        channels,
        color_space,
    };

    fill_y_plane_u16(image, channels, &mut planes.y);
    fill_uv_plane_u16(image, layout, tables, &mut planes.u, &mut planes.v);

    if let Some(alpha) = &mut planes.a {
        if alpha.len() >= PARALLEL_MIN_PIXELS {
            alpha
                .par_iter_mut()
                .zip(image.par_chunks_exact(channels))
                .for_each(|(dst, pixel)| *dst = down16_to_8(pixel[channels - 1]));
        } else {
            for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(channels)) {
                *dst = down16_to_8(pixel[channels - 1]);
            }
        }
    }

    Ok(planes)
}

fn fill_y_plane_u8(image: &[u8], channels: usize, y_plane: &mut [u8]) {
    let sfix = precision_shift(8);
    let coeffs = scale_matrix(&WEBP_RGB_TO_Y, 8);
    let convert = |dst: &mut u8, pixel: &[u8]| {
        *dst = rgb_to_component_8bit(
            import_u8(pixel[0]) as i32,
            import_u8(pixel[1]) as i32,
            import_u8(pixel[2]) as i32,
            &coeffs,
            sfix,
        );
    };
    if y_plane.len() >= PARALLEL_MIN_PIXELS {
        y_plane
            .par_iter_mut()
            .zip(image.par_chunks_exact(channels))
            .for_each(|(dst, pixel)| convert(dst, pixel));
    } else {
        for (dst, pixel) in y_plane.iter_mut().zip(image.chunks_exact(channels)) {
            convert(dst, pixel);
        }
    }
}

fn fill_y_plane_u16(image: &[u16], channels: usize, y_plane: &mut [u8]) {
    let bit_depth = effective_u16_bit_depth();
    let sfix = precision_shift(bit_depth);
    let coeffs = scale_matrix(&WEBP_RGB_TO_Y, bit_depth);
    let convert = |dst: &mut u8, pixel: &[u16]| {
        *dst = rgb_to_component_8bit(
            import_u16(pixel[0]) as i32,
            import_u16(pixel[1]) as i32,
            import_u16(pixel[2]) as i32,
            &coeffs,
            sfix,
        );
    };
    if y_plane.len() >= PARALLEL_MIN_PIXELS {
        y_plane
            .par_iter_mut()
            .zip(image.par_chunks_exact(channels))
            .for_each(|(dst, pixel)| convert(dst, pixel));
    } else {
        for (dst, pixel) in y_plane.iter_mut().zip(image.chunks_exact(channels)) {
            convert(dst, pixel);
        }
    }
}

fn fill_uv_plane_u8(
    image: &[u8],
    layout: SampleLayout,
    tables: &SimpleYuvTables,
    u_plane: &mut [u8],
    v_plane: &mut [u8],
) {
    let sfix = precision_shift(8);
    let uv_width = layout.width.div_ceil(2) as usize;
    let u_coeffs = scale_matrix(&WEBP_RGB_TO_U, 8);
    let v_coeffs = scale_matrix(&WEBP_RGB_TO_V, 8);

    let convert = |idx: usize, u: &mut u8, v: &mut u8| {
        let block_x = ((idx % uv_width) * 2) as u32;
        let block_y = ((idx / uv_width) * 2) as u32;
        let rgb = block_rgb_u8(image, layout, block_x, block_y, tables);
        *u = rgb_to_component_8bit(rgb[0] as i32, rgb[1] as i32, rgb[2] as i32, &u_coeffs, sfix);
        *v = rgb_to_component_8bit(rgb[0] as i32, rgb[1] as i32, rgb[2] as i32, &v_coeffs, sfix);
    };
    if u_plane.len() * 4 >= PARALLEL_MIN_PIXELS {
        u_plane
            .par_iter_mut()
            .zip(v_plane.par_iter_mut())
            .enumerate()
            .for_each(|(idx, (u, v))| convert(idx, u, v));
    } else {
        for (idx, (u, v)) in u_plane.iter_mut().zip(v_plane.iter_mut()).enumerate() {
            convert(idx, u, v);
        }
    }
}

fn fill_uv_plane_u16(
    image: &[u16],
    layout: SampleLayout,
    tables: &SimpleYuvTables,
    u_plane: &mut [u8],
    v_plane: &mut [u8],
) {
    let bit_depth = effective_u16_bit_depth();
    let sfix = precision_shift(bit_depth);
    let uv_width = layout.width.div_ceil(2) as usize;
    let u_coeffs = scale_matrix(&WEBP_RGB_TO_U, bit_depth);
    let v_coeffs = scale_matrix(&WEBP_RGB_TO_V, bit_depth);

    let convert = |idx: usize, u: &mut u8, v: &mut u8| {
        let block_x = ((idx % uv_width) * 2) as u32;
        let block_y = ((idx / uv_width) * 2) as u32;
        let rgb = block_rgb_u16(image, layout, block_x, block_y, tables);
        *u = rgb_to_component_8bit(rgb[0] as i32, rgb[1] as i32, rgb[2] as i32, &u_coeffs, sfix);
        *v = rgb_to_component_8bit(rgb[0] as i32, rgb[1] as i32, rgb[2] as i32, &v_coeffs, sfix);
    };
    if u_plane.len() * 4 >= PARALLEL_MIN_PIXELS {
        u_plane
            .par_iter_mut()
            .zip(v_plane.par_iter_mut())
            .enumerate()
            .for_each(|(idx, (u, v))| convert(idx, u, v));
    } else {
        for (idx, (u, v)) in u_plane.iter_mut().zip(v_plane.iter_mut()).enumerate() {
            convert(idx, u, v);
        }
    }
}

fn block_rgb_u8(
    image: &[u8],
    layout: SampleLayout,
    block_x: u32,
    block_y: u32,
    tables: &SimpleYuvTables,
) -> [u16; 3] {
    match layout.color_space {
        WorkingColorSpace::Source => average_source_block_u8(
            image,
            layout,
            block_x,
            block_y,
            &tables.source_u8_to_linear,
            &tables.linear_to_source_u8,
        ),
        WorkingColorSpace::LinearRgb => average_linear_block_u8(image, layout, block_x, block_y),
    }
}

fn block_rgb_u16(
    image: &[u16],
    layout: SampleLayout,
    block_x: u32,
    block_y: u32,
    tables: &SimpleYuvTables,
) -> [u16; 3] {
    match layout.color_space {
        WorkingColorSpace::Source => average_source_block_u16(
            image,
            layout,
            block_x,
            block_y,
            &tables.source_u16_to_linear,
            &tables.linear_to_source_u16,
        ),
        WorkingColorSpace::LinearRgb => average_linear_block_u16(image, layout, block_x, block_y),
    }
}

fn average_source_block_u8(
    image: &[u8],
    layout: SampleLayout,
    block_x: u32,
    block_y: u32,
    source_to_linear: &[Vec<u16>; 3],
    linear_to_source: &[Vec<u16>; 3],
) -> [u16; 3] {
    let mut sum_r = 0u32;
    let mut sum_g = 0u32;
    let mut sum_b = 0u32;

    for dy in 0..2 {
        for dx in 0..2 {
            let (px, py) = block_pixel(layout.width, layout.height, block_x, block_y, dx, dy);
            let idx = ((py * layout.width + px) as usize) * layout.channels;
            sum_r += source_to_linear[0][import_u8(image[idx]) as usize] as u32;
            sum_g += source_to_linear[1][import_u8(image[idx + 1]) as usize] as u32;
            sum_b += source_to_linear[2][import_u8(image[idx + 2]) as usize] as u32;
        }
    }

    [
        linear_to_source[0][average_linear(sum_r) as usize],
        linear_to_source[1][average_linear(sum_g) as usize],
        linear_to_source[2][average_linear(sum_b) as usize],
    ]
}

fn average_source_block_u16(
    image: &[u16],
    layout: SampleLayout,
    block_x: u32,
    block_y: u32,
    source_to_linear: &[Vec<u16>; 3],
    linear_to_source: &[Vec<u16>; 3],
) -> [u16; 3] {
    let mut sum_r = 0u32;
    let mut sum_g = 0u32;
    let mut sum_b = 0u32;

    for dy in 0..2 {
        for dx in 0..2 {
            let (px, py) = block_pixel(layout.width, layout.height, block_x, block_y, dx, dy);
            let idx = ((py * layout.width + px) as usize) * layout.channels;
            sum_r += source_to_linear[0][import_u16(image[idx]) as usize] as u32;
            sum_g += source_to_linear[1][import_u16(image[idx + 1]) as usize] as u32;
            sum_b += source_to_linear[2][import_u16(image[idx + 2]) as usize] as u32;
        }
    }

    [
        linear_to_source[0][average_linear(sum_r) as usize],
        linear_to_source[1][average_linear(sum_g) as usize],
        linear_to_source[2][average_linear(sum_b) as usize],
    ]
}

fn average_linear_block_u8(
    image: &[u8],
    layout: SampleLayout,
    block_x: u32,
    block_y: u32,
) -> [u16; 3] {
    let mut sum_r = 0u32;
    let mut sum_g = 0u32;
    let mut sum_b = 0u32;

    for dy in 0..2 {
        for dx in 0..2 {
            let (px, py) = block_pixel(layout.width, layout.height, block_x, block_y, dx, dy);
            let idx = ((py * layout.width + px) as usize) * layout.channels;
            sum_r += import_u8(image[idx]) as u32;
            sum_g += import_u8(image[idx + 1]) as u32;
            sum_b += import_u8(image[idx + 2]) as u32;
        }
    }

    [
        average_linear(sum_r),
        average_linear(sum_g),
        average_linear(sum_b),
    ]
}

fn average_linear_block_u16(
    image: &[u16],
    layout: SampleLayout,
    block_x: u32,
    block_y: u32,
) -> [u16; 3] {
    let mut sum_r = 0u32;
    let mut sum_g = 0u32;
    let mut sum_b = 0u32;

    for dy in 0..2 {
        for dx in 0..2 {
            let (px, py) = block_pixel(layout.width, layout.height, block_x, block_y, dx, dy);
            let idx = ((py * layout.width + px) as usize) * layout.channels;
            sum_r += import_u16(image[idx]) as u32;
            sum_g += import_u16(image[idx + 1]) as u32;
            sum_b += import_u16(image[idx + 2]) as u32;
        }
    }

    [
        average_linear(sum_r),
        average_linear(sum_g),
        average_linear(sum_b),
    ]
}

fn rgb_to_component_8bit(r: i32, g: i32, b: i32, coeffs: &[i32; 4], sfix: i32) -> u8 {
    let shift = YUV_FIX + sfix;
    let rounder = 1i64 << (shift - 1);
    let value = coeffs[0] as i64 * r as i64
        + coeffs[1] as i64 * g as i64
        + coeffs[2] as i64 * b as i64
        + coeffs[3] as i64
        + rounder;
    (value >> shift).clamp(0, 255) as u8
}

#[cfg(test)]
#[allow(dead_code)]
fn rgb_to_gray(r: i32, g: i32, b: i32) -> u16 {
    ((13933i64 * r as i64 + 46871i64 * g as i64 + 4732i64 * b as i64 + YUV_HALF) >> YUV_FIX) as u16
}

fn block_pixel(
    width: u32,
    height: u32,
    block_x: u32,
    block_y: u32,
    dx: u32,
    dy: u32,
) -> (u32, u32) {
    (
        block_x.saturating_add(dx).min(width - 1),
        block_y.saturating_add(dy).min(height - 1),
    )
}

fn average_linear(sum: u32) -> u16 {
    ((sum + 2) >> 2) as u16
}

fn precision_shift(rgb_bit_depth: u8) -> i32 {
    if rgb_bit_depth as i32 + 2 <= 14 {
        2
    } else {
        14 - rgb_bit_depth as i32
    }
}

fn scale_matrix(coeffs: &[i32; 4], rgb_bit_depth: u8) -> [i32; 4] {
    let rgb_max = (1u32 << rgb_bit_depth) - 1;
    let rgb_round = 1u32 << (rgb_bit_depth - 1);
    let yuv_max = 255u32;
    [
        ((coeffs[0] as i64 * yuv_max as i64 + rgb_round as i64) / rgb_max as i64) as i32,
        ((coeffs[1] as i64 * yuv_max as i64 + rgb_round as i64) / rgb_max as i64) as i32,
        ((coeffs[2] as i64 * yuv_max as i64 + rgb_round as i64) / rgb_max as i64) as i32,
        shift_i32(coeffs[3], precision_shift(rgb_bit_depth)),
    ]
}

fn shift_i32(value: i32, shift: i32) -> i32 {
    if shift >= 0 {
        value << shift
    } else {
        value >> -shift
    }
}

fn import_u8(value: u8) -> u16 {
    (value as u16) << precision_shift(8)
}

fn import_u16(value: u16) -> u16 {
    value >> (-precision_shift(16))
}

#[inline]
fn down16_to_8(value: u16) -> u8 {
    ((value as u32 * 255 + 32767) / 65535) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn srgb_to_linear_code(value: u16, bit_depth: u8) -> u16 {
        let max = ((1u32 << bit_depth) - 1) as f64;
        let value = value as f64 / max;
        let linear = if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        };
        (linear * u16::MAX as f64).round() as u16
    }

    fn linear_to_srgb_code(value: u16, bit_depth: u8) -> u16 {
        let max = ((1u32 << bit_depth) - 1) as f64;
        let value = value as f64 / u16::MAX as f64;
        let gamma = if value <= 0.0031308 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        };
        (gamma * max).round() as u16
    }

    #[test]
    fn missing_icc_defaults_to_srgb_for_linear_subsampling() {
        let image = WorkingImage {
            width: 2,
            height: 2,
            data: WorkingData::Rgba8(vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ]),
            color_space: WorkingColorSpace::Source,
        };
        let cms = ColorPipeline::new(None).unwrap();
        let planes = working_image_to_yuv420(&image, &cms).unwrap();

        let max = (1u16 << 10) - 1;
        let avg_r = linear_to_srgb_code(
            average_linear(
                srgb_to_linear_code(max, 10) as u32
                    + srgb_to_linear_code(0, 10) as u32
                    + srgb_to_linear_code(0, 10) as u32
                    + srgb_to_linear_code(max, 10) as u32,
            ),
            10,
        );
        let avg_g = linear_to_srgb_code(
            average_linear(
                srgb_to_linear_code(0, 10) as u32
                    + srgb_to_linear_code(max, 10) as u32
                    + srgb_to_linear_code(0, 10) as u32
                    + srgb_to_linear_code(max, 10) as u32,
            ),
            10,
        );
        let avg_b = linear_to_srgb_code(
            average_linear(
                srgb_to_linear_code(0, 10) as u32
                    + srgb_to_linear_code(0, 10) as u32
                    + srgb_to_linear_code(max, 10) as u32
                    + srgb_to_linear_code(max, 10) as u32,
            ),
            10,
        );

        assert_eq!(
            planes.u,
            vec![rgb_to_component_8bit(
                avg_r as i32,
                avg_g as i32,
                avg_b as i32,
                &scale_matrix(&WEBP_RGB_TO_U, 8),
                precision_shift(8),
            )]
        );
        assert_eq!(
            planes.v,
            vec![rgb_to_component_8bit(
                avg_r as i32,
                avg_g as i32,
                avg_b as i32,
                &scale_matrix(&WEBP_RGB_TO_V, 8),
                precision_shift(8),
            )]
        );
    }

    #[test]
    fn linear_rgb_input_subsamples_without_gamma_roundtrip() {
        let cms = ColorPipeline::new(None).unwrap();
        let image = WorkingImage {
            width: 2,
            height: 2,
            data: WorkingData::Rgba8(vec![
                32, 64, 96, 255, 64, 96, 128, 255, 96, 128, 160, 255, 128, 160, 192, 255,
            ]),
            color_space: WorkingColorSpace::LinearRgb,
        };
        let planes = working_image_to_yuv420(&image, &cms).unwrap();

        let y_coeffs = scale_matrix(&WEBP_RGB_TO_Y, 8);
        let u_coeffs = scale_matrix(&WEBP_RGB_TO_U, 8);
        let v_coeffs = scale_matrix(&WEBP_RGB_TO_V, 8);

        assert_eq!(
            planes.y[0],
            rgb_to_component_8bit(
                import_u8(32) as i32,
                import_u8(64) as i32,
                import_u8(96) as i32,
                &y_coeffs,
                precision_shift(8),
            )
        );
        assert_eq!(
            planes.u,
            vec![rgb_to_component_8bit(
                average_linear(
                    import_u8(32) as u32
                        + import_u8(64) as u32
                        + import_u8(96) as u32
                        + import_u8(128) as u32,
                ) as i32,
                average_linear(
                    import_u8(64) as u32
                        + import_u8(96) as u32
                        + import_u8(128) as u32
                        + import_u8(160) as u32,
                ) as i32,
                average_linear(
                    import_u8(96) as u32
                        + import_u8(128) as u32
                        + import_u8(160) as u32
                        + import_u8(192) as u32,
                ) as i32,
                &u_coeffs,
                precision_shift(8),
            )]
        );
        assert_eq!(
            planes.v,
            vec![rgb_to_component_8bit(
                average_linear(
                    import_u8(32) as u32
                        + import_u8(64) as u32
                        + import_u8(96) as u32
                        + import_u8(128) as u32,
                ) as i32,
                average_linear(
                    import_u8(64) as u32
                        + import_u8(96) as u32
                        + import_u8(128) as u32
                        + import_u8(160) as u32,
                ) as i32,
                average_linear(
                    import_u8(96) as u32
                        + import_u8(128) as u32
                        + import_u8(160) as u32
                        + import_u8(192) as u32,
                ) as i32,
                &v_coeffs,
                precision_shift(8),
            )]
        );
    }
}

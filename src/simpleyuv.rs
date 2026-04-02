use webpx::YuvPlanes;

use crate::cms::{ResizeColorPipeline, SimpleYuvTables};
use crate::decode::{WorkingColorSpace, WorkingData, WorkingImage};
use crate::error::Result;

const YUV_FIX: i32 = 16;
const YUV_HALF: i64 = 1 << (YUV_FIX - 1);
const WEBP_RGB_TO_Y: [i32; 4] = [16839, 33059, 6420, 16 << 16];
const WEBP_RGB_TO_U: [i32; 4] = [-9719, -19081, 28800, 128 << 16];
const WEBP_RGB_TO_V: [i32; 4] = [28800, -24116, -4684, 128 << 16];

pub fn rgba_to_yuv420(image: &WorkingImage, cms: &ResizeColorPipeline) -> Result<YuvPlanes> {
    let tables = cms.simpleyuv_tables()?;
    match &image.data {
        WorkingData::U8(data) => {
            rgba8_to_yuv420(data, image.width, image.height, image.color_space, &tables)
        }
        WorkingData::U16(data) => {
            rgba16_to_yuv420(data, image.width, image.height, image.color_space, &tables)
        }
    }
}

pub fn rgba_to_yuv420_sharpish_linear_u16(image: &WorkingImage) -> Option<Result<YuvPlanes>> {
    if image.color_space != WorkingColorSpace::LinearRgb {
        return None;
    }

    let WorkingData::U16(data) = &image.data else {
        return None;
    };

    Some(rgba16_to_yuv420_sharpish_linear(
        data,
        image.width,
        image.height,
    ))
}

fn effective_u16_bit_depth() -> u8 {
    (16i32 + precision_shift(16)) as u8
}

fn rgba16_to_yuv420_sharpish_linear(image: &[u16], width: u32, height: u32) -> Result<YuvPlanes> {
    let with_alpha = has_alpha_u16(image);
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
            let idx = (y * w + x) * 4;
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
                    let idx = ((py * width + px) as usize) * 4;
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
        for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(4)) {
            *dst = down16_to_8(pixel[3]);
        }
    }

    Ok(planes)
}

fn rgba8_to_yuv420(
    image: &[u8],
    width: u32,
    height: u32,
    color_space: WorkingColorSpace,
    tables: &SimpleYuvTables,
) -> Result<YuvPlanes> {
    let with_alpha = has_alpha_u8(image);
    let mut planes = YuvPlanes::new(width, height, with_alpha);

    fill_y_plane_u8(image, &mut planes.y);
    fill_uv_plane_u8(
        image,
        width,
        height,
        color_space,
        tables,
        &mut planes.u,
        &mut planes.v,
    );

    if let Some(alpha) = &mut planes.a {
        for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(4)) {
            *dst = pixel[3];
        }
    }

    Ok(planes)
}

fn rgba16_to_yuv420(
    image: &[u16],
    width: u32,
    height: u32,
    color_space: WorkingColorSpace,
    tables: &SimpleYuvTables,
) -> Result<YuvPlanes> {
    let with_alpha = has_alpha_u16(image);
    let mut planes = YuvPlanes::new(width, height, with_alpha);

    fill_y_plane_u16(image, &mut planes.y);
    fill_uv_plane_u16(
        image,
        width,
        height,
        color_space,
        tables,
        &mut planes.u,
        &mut planes.v,
    );

    if let Some(alpha) = &mut planes.a {
        for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(4)) {
            *dst = down16_to_8(pixel[3]);
        }
    }

    Ok(planes)
}

fn fill_y_plane_u8(image: &[u8], y_plane: &mut [u8]) {
    let sfix = precision_shift(8);
    let coeffs = scale_matrix(&WEBP_RGB_TO_Y, 8);
    for (dst, pixel) in y_plane.iter_mut().zip(image.chunks_exact(4)) {
        *dst = rgb_to_component_8bit(
            import_u8(pixel[0]) as i32,
            import_u8(pixel[1]) as i32,
            import_u8(pixel[2]) as i32,
            &coeffs,
            sfix,
        );
    }
}

fn fill_y_plane_u16(image: &[u16], y_plane: &mut [u8]) {
    let bit_depth = effective_u16_bit_depth();
    let sfix = precision_shift(bit_depth);
    let coeffs = scale_matrix(&WEBP_RGB_TO_Y, bit_depth);
    for (dst, pixel) in y_plane.iter_mut().zip(image.chunks_exact(4)) {
        *dst = rgb_to_component_8bit(
            import_u16(pixel[0]) as i32,
            import_u16(pixel[1]) as i32,
            import_u16(pixel[2]) as i32,
            &coeffs,
            sfix,
        );
    }
}

fn fill_uv_plane_u8(
    image: &[u8],
    width: u32,
    height: u32,
    color_space: WorkingColorSpace,
    tables: &SimpleYuvTables,
    u_plane: &mut [u8],
    v_plane: &mut [u8],
) {
    let sfix = precision_shift(8);
    let uv_width = (width + 1) / 2;
    let u_coeffs = scale_matrix(&WEBP_RGB_TO_U, 8);
    let v_coeffs = scale_matrix(&WEBP_RGB_TO_V, 8);

    for block_y in (0..height).step_by(2) {
        for block_x in (0..width).step_by(2) {
            let rgb = match color_space {
                WorkingColorSpace::Source => average_source_block_u8(
                    image,
                    width,
                    height,
                    block_x,
                    block_y,
                    &tables.source_u8_to_linear,
                    &tables.linear_to_source_u8,
                ),
                WorkingColorSpace::LinearRgb => {
                    average_linear_block_u8(image, width, height, block_x, block_y)
                }
            };
            let idx = ((block_y / 2) * uv_width + (block_x / 2)) as usize;
            u_plane[idx] =
                rgb_to_component_8bit(rgb[0] as i32, rgb[1] as i32, rgb[2] as i32, &u_coeffs, sfix);
            v_plane[idx] =
                rgb_to_component_8bit(rgb[0] as i32, rgb[1] as i32, rgb[2] as i32, &v_coeffs, sfix);
        }
    }
}

fn fill_uv_plane_u16(
    image: &[u16],
    width: u32,
    height: u32,
    color_space: WorkingColorSpace,
    tables: &SimpleYuvTables,
    u_plane: &mut [u8],
    v_plane: &mut [u8],
) {
    let bit_depth = effective_u16_bit_depth();
    let sfix = precision_shift(bit_depth);
    let uv_width = (width + 1) / 2;
    let u_coeffs = scale_matrix(&WEBP_RGB_TO_U, bit_depth);
    let v_coeffs = scale_matrix(&WEBP_RGB_TO_V, bit_depth);

    for block_y in (0..height).step_by(2) {
        for block_x in (0..width).step_by(2) {
            let rgb = match color_space {
                WorkingColorSpace::Source => average_source_block_u16(
                    image,
                    width,
                    height,
                    block_x,
                    block_y,
                    &tables.source_u16_to_linear,
                    &tables.linear_to_source_u16,
                ),
                WorkingColorSpace::LinearRgb => {
                    average_linear_block_u16(image, width, height, block_x, block_y)
                }
            };
            let idx = ((block_y / 2) * uv_width + (block_x / 2)) as usize;
            u_plane[idx] =
                rgb_to_component_8bit(rgb[0] as i32, rgb[1] as i32, rgb[2] as i32, &u_coeffs, sfix);
            v_plane[idx] =
                rgb_to_component_8bit(rgb[0] as i32, rgb[1] as i32, rgb[2] as i32, &v_coeffs, sfix);
        }
    }
}

fn average_source_block_u8(
    image: &[u8],
    width: u32,
    height: u32,
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
            let (px, py) = block_pixel(width, height, block_x, block_y, dx, dy);
            let idx = ((py * width + px) as usize) * 4;
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
    width: u32,
    height: u32,
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
            let (px, py) = block_pixel(width, height, block_x, block_y, dx, dy);
            let idx = ((py * width + px) as usize) * 4;
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
    width: u32,
    height: u32,
    block_x: u32,
    block_y: u32,
) -> [u16; 3] {
    let mut sum_r = 0u32;
    let mut sum_g = 0u32;
    let mut sum_b = 0u32;

    for dy in 0..2 {
        for dx in 0..2 {
            let (px, py) = block_pixel(width, height, block_x, block_y, dx, dy);
            let idx = ((py * width + px) as usize) * 4;
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
    width: u32,
    height: u32,
    block_x: u32,
    block_y: u32,
) -> [u16; 3] {
    let mut sum_r = 0u32;
    let mut sum_g = 0u32;
    let mut sum_b = 0u32;

    for dy in 0..2 {
        for dx in 0..2 {
            let (px, py) = block_pixel(width, height, block_x, block_y, dx, dy);
            let idx = ((py * width + px) as usize) * 4;
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
fn has_alpha_u8(image: &[u8]) -> bool {
    image.chunks_exact(4).any(|pixel| pixel[3] != u8::MAX)
}

#[inline]
fn has_alpha_u16(image: &[u16]) -> bool {
    image.chunks_exact(4).any(|pixel| pixel[3] != u16::MAX)
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
            data: WorkingData::U8(vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ]),
            color_space: WorkingColorSpace::Source,
        };
        let cms = ResizeColorPipeline::new(None).unwrap();
        let planes = rgba_to_yuv420(&image, &cms).unwrap();

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
        let cms = ResizeColorPipeline::new(None).unwrap();
        let image = WorkingImage {
            width: 2,
            height: 2,
            data: WorkingData::U8(vec![
                32, 64, 96, 255, 64, 96, 128, 255, 96, 128, 160, 255, 128, 160, 192, 255,
            ]),
            color_space: WorkingColorSpace::LinearRgb,
        };
        let planes = rgba_to_yuv420(&image, &cms).unwrap();

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

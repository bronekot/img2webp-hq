use webpx::YuvPlanes;

use crate::error::Result;

pub fn rgba8_to_yuv420(image: &[u8], width: u32, height: u32) -> Result<YuvPlanes> {
    let with_alpha = image.chunks_exact(4).any(|pixel| pixel[3] != u8::MAX);
    let mut planes = YuvPlanes::new(width, height, with_alpha);

    for (row_idx, row) in planes.y.chunks_mut(width as usize).enumerate() {
        for (col_idx, y_val) in row.iter_mut().enumerate() {
            let px = col_idx as u32;
            let py = row_idx as u32;
            let idx = ((py * width + px) as usize) * 4;
            *y_val = rgb_to_y(image[idx], image[idx + 1], image[idx + 2]);
        }
    }

    let mut u_block = [[0u8; 2]; 2];
    let mut v_block = [[0u8; 2]; 2];

    for block_y in (0..height).step_by(2) {
        for block_x in (0..width).step_by(2) {
            for dy in 0..2 {
                for dx in 0..2 {
                    let px = block_x.saturating_add(dx).min(width - 1);
                    let py = block_y.saturating_add(dy).min(height - 1);
                    let idx = ((py * width + px) as usize) * 4;
                    u_block[dy as usize][dx as usize] =
                        rgb_to_u(image[idx], image[idx + 1], image[idx + 2]);
                    v_block[dy as usize][dx as usize] =
                        rgb_to_v(image[idx], image[idx + 1], image[idx + 2]);
                }
            }
            let avg_u: u8 = average_u8_to_u8(&u_block);
            let avg_v: u8 = average_u8_to_u8(&v_block);

            let uv_width = (width + 1) / 2;
            let uv_x = block_x / 2;
            let uv_y = block_y / 2;
            let idx = (uv_y * uv_width + uv_x) as usize;
            if idx < planes.u.len() {
                planes.u[idx] = avg_u;
            }
            if idx < planes.v.len() {
                planes.v[idx] = avg_v;
            }
        }
    }

    if let Some(alpha) = &mut planes.a {
        for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(4)) {
            *dst = pixel[3];
        }
    }

    Ok(planes)
}

pub fn rgba16_to_yuv420(image: &[u16], width: u32, height: u32) -> Result<YuvPlanes> {
    let with_alpha = image.chunks_exact(4).any(|pixel| pixel[3] != u16::MAX);
    let mut planes = YuvPlanes::new(width, height, with_alpha);

    for (row_idx, row) in planes.y.chunks_mut(width as usize).enumerate() {
        for (col_idx, y_val) in row.iter_mut().enumerate() {
            let px = col_idx as u32;
            let py = row_idx as u32;
            let idx = ((py * width + px) as usize) * 4;
            *y_val = rgb_to_y(
                down16_to_8(image[idx]),
                down16_to_8(image[idx + 1]),
                down16_to_8(image[idx + 2]),
            );
        }
    }

    for block_y in (0..height).step_by(2) {
        for block_x in (0..width).step_by(2) {
            let mut sum_u: u32 = 0;
            let mut sum_v: u32 = 0;
            let mut count: u32 = 0;

            for dy in 0..2 {
                for dx in 0..2 {
                    let px = block_x.saturating_add(dx).min(width - 1);
                    let py = block_y.saturating_add(dy).min(height - 1);
                    let idx = ((py * width + px) as usize) * 4;
                    sum_u += rgb_to_u(
                        down16_to_8(image[idx]),
                        down16_to_8(image[idx + 1]),
                        down16_to_8(image[idx + 2]),
                    ) as u32;
                    sum_v += rgb_to_v(
                        down16_to_8(image[idx]),
                        down16_to_8(image[idx + 1]),
                        down16_to_8(image[idx + 2]),
                    ) as u32;
                    count += 1;
                }
            }

            let avg_u = (sum_u / count) as u8;
            let avg_v = (sum_v / count) as u8;

            let uv_width = (width + 1) / 2;
            let uv_x = block_x / 2;
            let uv_y = block_y / 2;
            let idx = (uv_y * uv_width + uv_x) as usize;
            if idx < planes.u.len() {
                planes.u[idx] = avg_u;
            }
            if idx < planes.v.len() {
                planes.v[idx] = avg_v;
            }
        }
    }

    if let Some(alpha) = &mut planes.a {
        for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(4)) {
            *dst = down16_to_8(pixel[3]);
        }
    }

    Ok(planes)
}

#[inline]
fn rgb_to_y(r: u8, g: u8, b: u8) -> u8 {
    let y = 66 * r as i32 + 129 * g as i32 + 25 * b as i32 + 128;
    ((y >> 8) + 16).clamp(16, 235) as u8
}

#[inline]
fn rgb_to_u(r: u8, g: u8, b: u8) -> u8 {
    let u = -38 * r as i32 - 74 * g as i32 + 112 * b as i32 + 128;
    (((u >> 8) + 128) as u8).clamp(16, 240)
}

#[inline]
fn rgb_to_v(r: u8, g: u8, b: u8) -> u8 {
    let v = 112 * r as i32 - 94 * g as i32 - 18 * b as i32 + 128;
    (((v >> 8) + 128) as u8).clamp(16, 240)
}

#[inline]
fn average_u8_to_u8(block: &[[u8; 2]; 2]) -> u8 {
    let sum: u32 = block
        .iter()
        .flat_map(|row| row.iter())
        .map(|&v| v as u32)
        .sum();
    ((sum + 2) / 4) as u8
}

#[inline]
fn down16_to_8(value: u16) -> u8 {
    ((value as u32 * 255 + 32767) / 65535) as u8
}

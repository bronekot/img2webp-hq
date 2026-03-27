use std::ffi::c_void;

use webpx::YuvPlanes;

use crate::error::{Error, Result};

#[repr(C)]
struct SharpYuvConversionMatrix {
    rgb_to_y: [i32; 4],
    rgb_to_u: [i32; 4],
    rgb_to_v: [i32; 4],
}

#[repr(C)]
struct SharpYuvOptions {
    yuv_matrix: *const SharpYuvConversionMatrix,
    transfer_type: SharpYuvTransferFunctionType,
}

#[repr(C)]
#[derive(Clone, Copy)]
enum SharpYuvTransferFunctionType {
    Srgb = 13,
    Linear = 8,
}

#[repr(C)]
#[derive(Clone, Copy)]
enum SharpYuvMatrixType {
    Webp = 0,
}

unsafe extern "C" {
    fn SharpYuvGetConversionMatrix(
        matrix_type: SharpYuvMatrixType,
    ) -> *const SharpYuvConversionMatrix;
    fn SharpYuvOptionsInitInternal(
        yuv_matrix: *const SharpYuvConversionMatrix,
        options: *mut SharpYuvOptions,
        version: i32,
    ) -> i32;
    fn SharpYuvConvertWithOptions(
        r_ptr: *const c_void,
        g_ptr: *const c_void,
        b_ptr: *const c_void,
        rgb_step: i32,
        rgb_stride: i32,
        rgb_bit_depth: i32,
        y_ptr: *mut c_void,
        y_stride: i32,
        u_ptr: *mut c_void,
        u_stride: i32,
        v_ptr: *mut c_void,
        v_stride: i32,
        yuv_bit_depth: i32,
        width: i32,
        height: i32,
        options: *const SharpYuvOptions,
    ) -> i32;
}

const SHARPYUV_VERSION: i32 = (0 << 24) | (4 << 16) | 1;

pub fn rgba8_to_yuv420(
    image: &[u8],
    width: u32,
    height: u32,
    assume_linear: bool,
) -> Result<YuvPlanes> {
    convert_to_yuv420(
        width,
        height,
        has_alpha_u8(image),
        if assume_linear {
            SharpYuvTransferFunctionType::Linear
        } else {
            SharpYuvTransferFunctionType::Srgb
        },
        image.as_ptr() as *const c_void,
        unsafe { image.as_ptr().add(1) } as *const c_void,
        unsafe { image.as_ptr().add(2) } as *const c_void,
        4,
        (width * 4) as i32,
        8,
        |alpha| {
            for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(4)) {
                *dst = pixel[3];
            }
        },
    )
}

pub fn rgba16_to_yuv420(
    image: &[u16],
    width: u32,
    height: u32,
    assume_linear: bool,
) -> Result<YuvPlanes> {
    let bytes = bytemuck::cast_slice::<u16, u8>(image);
    convert_to_yuv420(
        width,
        height,
        has_alpha_u16(image),
        if assume_linear {
            SharpYuvTransferFunctionType::Linear
        } else {
            SharpYuvTransferFunctionType::Srgb
        },
        bytes.as_ptr() as *const c_void,
        unsafe { bytes.as_ptr().add(2) } as *const c_void,
        unsafe { bytes.as_ptr().add(4) } as *const c_void,
        8,
        (width * 8) as i32,
        16,
        |alpha| {
            for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(4)) {
                *dst = down16_to_8(pixel[3]);
            }
        },
    )
}

fn convert_to_yuv420(
    width: u32,
    height: u32,
    with_alpha: bool,
    transfer_type: SharpYuvTransferFunctionType,
    r_ptr: *const c_void,
    g_ptr: *const c_void,
    b_ptr: *const c_void,
    rgb_step: i32,
    rgb_stride: i32,
    rgb_bit_depth: i32,
    fill_alpha: impl FnOnce(&mut [u8]),
) -> Result<YuvPlanes> {
    let mut planes = YuvPlanes::new(width, height, with_alpha);

    let matrix = unsafe { SharpYuvGetConversionMatrix(SharpYuvMatrixType::Webp) };
    if matrix.is_null() {
        return Err(Error::encode("failed to obtain SharpYUV conversion matrix"));
    }

    let mut options = SharpYuvOptions {
        yuv_matrix: matrix,
        transfer_type,
    };
    let init_ok = unsafe { SharpYuvOptionsInitInternal(matrix, &mut options, SHARPYUV_VERSION) };
    if init_ok == 0 {
        return Err(Error::encode("failed to initialize SharpYUV options"));
    }

    let ok = unsafe {
        SharpYuvConvertWithOptions(
            r_ptr,
            g_ptr,
            b_ptr,
            rgb_step,
            rgb_stride,
            rgb_bit_depth,
            planes.y.as_mut_ptr() as *mut c_void,
            planes.y_stride as i32,
            planes.u.as_mut_ptr() as *mut c_void,
            planes.u_stride as i32,
            planes.v.as_mut_ptr() as *mut c_void,
            planes.v_stride as i32,
            8,
            width as i32,
            height as i32,
            &options,
        )
    };
    if ok == 0 {
        return Err(Error::encode("SharpYUV conversion failed"));
    }

    if let Some(alpha) = &mut planes.a {
        fill_alpha(alpha);
    }

    Ok(planes)
}

fn has_alpha_u8(image: &[u8]) -> bool {
    image.chunks_exact(4).any(|pixel| pixel[3] != u8::MAX)
}

fn has_alpha_u16(image: &[u16]) -> bool {
    image.chunks_exact(4).any(|pixel| pixel[3] != u16::MAX)
}

fn down16_to_8(value: u16) -> u8 {
    ((value as u32 * 255 + 32767) / 65535) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_white_keeps_neutral_chroma_across_input_bit_depths() {
        let rgba8 = vec![
            255u8, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        ];
        let rgba16 = vec![
            65535u16, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535,
            65535, 65535, 65535, 65535, 65535,
        ];

        let yuv8 = rgba8_to_yuv420(&rgba8, 2, 2, true).unwrap();
        let yuv16 = rgba16_to_yuv420(&rgba16, 2, 2, true).unwrap();

        assert_eq!(yuv8.y, vec![235, 235, 235, 235]);
        assert_eq!(yuv16.y, vec![236, 236, 236, 236]);
        assert_eq!(yuv8.u, vec![128]);
        assert_eq!(yuv8.v, vec![128]);
        assert_eq!(yuv16.u, vec![128]);
        assert_eq!(yuv16.v, vec![128]);
    }
}

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

pub fn rgba16_to_yuv420(
    image: &[u16],
    width: u32,
    height: u32,
    assume_linear: bool,
) -> Result<YuvPlanes> {
    let mut planes = YuvPlanes::new(width, height, has_alpha(image));

    let matrix = unsafe { SharpYuvGetConversionMatrix(SharpYuvMatrixType::Webp) };
    if matrix.is_null() {
        return Err(Error::encode("failed to obtain SharpYUV conversion matrix"));
    }

    let mut options = SharpYuvOptions {
        yuv_matrix: matrix,
        transfer_type: if assume_linear {
            SharpYuvTransferFunctionType::Linear
        } else {
            SharpYuvTransferFunctionType::Srgb
        },
    };
    let init_ok = unsafe { SharpYuvOptionsInitInternal(matrix, &mut options, SHARPYUV_VERSION) };
    if init_ok == 0 {
        return Err(Error::encode("failed to initialize SharpYUV options"));
    }

    let bytes = bytemuck::cast_slice::<u16, u8>(image);
    let rgb_step = 8i32;
    let rgb_stride = (width * 8) as i32;
    let r_ptr = bytes.as_ptr() as *const c_void;
    let g_ptr = unsafe { bytes.as_ptr().add(2) } as *const c_void;
    let b_ptr = unsafe { bytes.as_ptr().add(4) } as *const c_void;

    let ok = unsafe {
        SharpYuvConvertWithOptions(
            r_ptr,
            g_ptr,
            b_ptr,
            rgb_step,
            rgb_stride,
            16,
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
        for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(4)) {
            *dst = down16_to_8(pixel[3]);
        }
    }

    Ok(planes)
}

fn has_alpha(image: &[u16]) -> bool {
    image.chunks_exact(4).any(|pixel| pixel[3] != u16::MAX)
}

fn down16_to_8(value: u16) -> u8 {
    ((value as u32 * 255 + 32767) / 65535) as u8
}

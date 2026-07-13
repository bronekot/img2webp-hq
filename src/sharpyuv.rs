use std::ffi::c_void;

use webpx::YuvPlanes;

use crate::decode::{WorkingData, WorkingImage};
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

const SHARPYUV_VERSION: i32 = (4 << 16) | 1;

#[derive(Clone, Copy)]
struct RgbInput {
    r: *const c_void,
    g: *const c_void,
    b: *const c_void,
    step: i32,
    stride: i32,
    bit_depth: i32,
}

pub fn working_image_to_yuv420(image: &WorkingImage, assume_linear: bool) -> Result<YuvPlanes> {
    let transfer = if assume_linear {
        SharpYuvTransferFunctionType::Linear
    } else {
        SharpYuvTransferFunctionType::Srgb
    };
    match &image.data {
        WorkingData::Rgb8(data) => convert_to_yuv420(
            image.width,
            image.height,
            false,
            transfer,
            RgbInput {
                r: data.as_ptr() as *const c_void,
                g: unsafe { data.as_ptr().add(1) } as *const c_void,
                b: unsafe { data.as_ptr().add(2) } as *const c_void,
                step: 3,
                stride: (image.width * 3) as i32,
                bit_depth: 8,
            },
            |_| {},
        ),
        WorkingData::Rgba8(data) => convert_to_yuv420(
            image.width,
            image.height,
            true,
            transfer,
            RgbInput {
                r: data.as_ptr() as *const c_void,
                g: unsafe { data.as_ptr().add(1) } as *const c_void,
                b: unsafe { data.as_ptr().add(2) } as *const c_void,
                step: 4,
                stride: (image.width * 4) as i32,
                bit_depth: 8,
            },
            |alpha| {
                for (dst, pixel) in alpha.iter_mut().zip(data.chunks_exact(4)) {
                    *dst = pixel[3];
                }
            },
        ),
        WorkingData::Rgb16(data) => {
            convert_u16_image(data, image.width, image.height, false, transfer, 3)
        }
        WorkingData::Rgba16(data) => {
            convert_u16_image(data, image.width, image.height, true, transfer, 4)
        }
    }
}

fn convert_u16_image(
    image: &[u16],
    width: u32,
    height: u32,
    with_alpha: bool,
    transfer: SharpYuvTransferFunctionType,
    channels: usize,
) -> Result<YuvPlanes> {
    let bytes = bytemuck::cast_slice::<u16, u8>(image);
    convert_to_yuv420(
        width,
        height,
        with_alpha,
        transfer,
        RgbInput {
            r: bytes.as_ptr() as *const c_void,
            g: unsafe { bytes.as_ptr().add(2) } as *const c_void,
            b: unsafe { bytes.as_ptr().add(4) } as *const c_void,
            step: (channels * 2) as i32,
            stride: (width as usize * channels * 2) as i32,
            bit_depth: 16,
        },
        |alpha| {
            for (dst, pixel) in alpha.iter_mut().zip(image.chunks_exact(channels)) {
                *dst = down16_to_8(pixel[channels - 1]);
            }
        },
    )
}

fn convert_to_yuv420(
    width: u32,
    height: u32,
    with_alpha: bool,
    transfer_type: SharpYuvTransferFunctionType,
    input: RgbInput,
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
            input.r,
            input.g,
            input.b,
            input.step,
            input.stride,
            input.bit_depth,
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

fn down16_to_8(value: u16) -> u8 {
    ((value as u32 * 255 + 32767) / 65535) as u8
}

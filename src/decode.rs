use std::fs;
use std::io::Cursor;
use std::path::Path;

use image::codecs::jpeg::JpegDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::metadata::Orientation;
use image::{ColorType, DynamicImage, ImageBuffer, ImageDecoder, ImageFormat, Rgba};
use jpegli::{DecodedImage, DecodedImage16, Decoder, DecoderConfig, PixelLayout};

use crate::error::{Error, Result};
use crate::metadata::SourceMetadata;

#[derive(Debug, Clone)]
pub enum WorkingData {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingColorSpace {
    Source,
    LinearRgb,
}

#[derive(Debug, Clone)]
pub struct WorkingImage {
    pub width: u32,
    pub height: u32,
    pub data: WorkingData,
    pub color_space: WorkingColorSpace,
}

#[derive(Debug, Clone)]
pub struct DecodedInput {
    pub image: WorkingImage,
    pub metadata: SourceMetadata,
}

impl WorkingImage {
    pub fn apply_orientation(self, orientation: Orientation) -> Result<Self> {
        if orientation == Orientation::NoTransforms {
            return Ok(self);
        }

        let color_space = self.color_space;
        match self.data {
            WorkingData::U8(data) => {
                let buffer =
                    ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(self.width, self.height, data)
                        .ok_or_else(|| {
                            Error::decode("failed to build RGBA8 image for orientation")
                        })?;
                let mut dynamic = DynamicImage::ImageRgba8(buffer);
                dynamic.apply_orientation(orientation);
                let rotated = dynamic.into_rgba8();
                Ok(Self {
                    width: rotated.width(),
                    height: rotated.height(),
                    data: WorkingData::U8(rotated.into_raw()),
                    color_space,
                })
            }
            WorkingData::U16(data) => {
                let buffer =
                    ImageBuffer::<Rgba<u16>, Vec<u16>>::from_raw(self.width, self.height, data)
                        .ok_or_else(|| {
                            Error::decode("failed to build RGBA16 image for orientation")
                        })?;
                let mut dynamic = DynamicImage::ImageRgba16(buffer);
                dynamic.apply_orientation(orientation);
                let rotated = dynamic.into_rgba16();
                Ok(Self {
                    width: rotated.width(),
                    height: rotated.height(),
                    data: WorkingData::U16(rotated.into_raw()),
                    color_space,
                })
            }
        }
    }
}

pub fn decode(path: &Path, fast: bool) -> Result<DecodedInput> {
    let bytes = fs::read(path)?;
    let format = image::guess_format(&bytes).map_err(Error::from)?;

    match format {
        ImageFormat::Jpeg => decode_jpeg(&bytes, fast),
        ImageFormat::Png => decode_png(&bytes, fast),
        ImageFormat::WebP => decode_webp(&bytes, fast),
        _ => Err(Error::unsupported(format!(
            "unsupported input format: {:?}",
            format
        ))),
    }
}

fn decode_jpeg(bytes: &[u8], fast: bool) -> Result<DecodedInput> {
    let metadata = read_jpeg_metadata(bytes)?;
    let decoder = Decoder::new(DecoderConfig {
        output_format: None,
    })
    .map_err(|err| Error::decode(err.to_string()))?;

    let image = if fast {
        let decoded = decoder
            .decode(bytes)
            .map_err(|err| Error::decode(err.to_string()))?;
        working_image_from_jpeg_u8(decoded)?
    } else {
        let decoded = decoder
            .decode_u16(bytes)
            .map_err(|err| Error::decode(err.to_string()))?;
        working_image_from_jpeg_u16(decoded)?
    };

    finish_decoded(image, metadata)
}

fn decode_png(bytes: &[u8], fast: bool) -> Result<DecodedInput> {
    let reader = Cursor::new(bytes);
    let mut decoder = PngDecoder::new(reader)?;
    if decoder.is_apng()? {
        return Err(Error::unsupported(
            "animated PNG input is not supported in v1",
        ));
    }
    let metadata = extract_metadata(&mut decoder)?;
    let image = working_image_from_decoder(decoder, fast)?;
    finish_decoded(image, metadata)
}

fn decode_webp(bytes: &[u8], fast: bool) -> Result<DecodedInput> {
    let reader = Cursor::new(bytes);
    let mut decoder = WebPDecoder::new(reader)?;
    if decoder.has_animation() {
        return Err(Error::unsupported(
            "animated WebP input is not supported in v1",
        ));
    }
    let metadata = extract_metadata(&mut decoder)?;
    let image = working_image_from_decoder(decoder, fast)?;
    finish_decoded(image, metadata)
}

fn finish_decoded(image: WorkingImage, mut metadata: SourceMetadata) -> Result<DecodedInput> {
    let orientation = metadata.orientation;
    metadata.normalize_orientation();
    let image = image.apply_orientation(orientation)?;
    Ok(DecodedInput { image, metadata })
}

fn read_jpeg_metadata(bytes: &[u8]) -> Result<SourceMetadata> {
    let cursor = Cursor::new(bytes);
    let mut decoder = JpegDecoder::new(cursor)?;
    extract_metadata(&mut decoder)
}

fn extract_metadata(decoder: &mut dyn ImageDecoder) -> Result<SourceMetadata> {
    let orientation = decoder.orientation()?;
    Ok(SourceMetadata {
        icc: decoder.icc_profile()?,
        exif: decoder.exif_metadata()?,
        xmp: decoder.xmp_metadata()?,
        orientation,
    })
}

fn working_image_from_jpeg_u8(decoded: DecodedImage) -> Result<WorkingImage> {
    let samples = match decoded.format {
        PixelLayout::Gray => gray8_to_rgba8(&decoded.data),
        PixelLayout::Rgb => rgb8_to_rgba8(&decoded.data),
        PixelLayout::Rgba => rgba8_to_rgba8(&decoded.data),
    };

    Ok(WorkingImage {
        width: decoded.width,
        height: decoded.height,
        data: WorkingData::U8(samples),
        color_space: WorkingColorSpace::Source,
    })
}

fn working_image_from_jpeg_u16(decoded: DecodedImage16) -> Result<WorkingImage> {
    let samples = match decoded.format {
        PixelLayout::Gray => gray16_to_rgba16(&decoded.data),
        PixelLayout::Rgb => rgb16_to_rgba16(&decoded.data),
        PixelLayout::Rgba => rgba16_to_rgba16(&decoded.data),
    };

    Ok(WorkingImage {
        width: decoded.width,
        height: decoded.height,
        data: WorkingData::U16(samples),
        color_space: WorkingColorSpace::Source,
    })
}

fn working_image_from_decoder<D: ImageDecoder>(decoder: D, fast: bool) -> Result<WorkingImage> {
    let (width, height) = decoder.dimensions();
    let color_type = decoder.color_type();
    let mut buf = vec![0u8; decoder.total_bytes() as usize];
    decoder.read_image(&mut buf)?;

    if fast {
        convert_decoded_to_rgba8(width, height, color_type, &buf)
    } else {
        convert_decoded_to_rgba16(width, height, color_type, &buf)
    }
}

fn convert_decoded_to_rgba8(
    width: u32,
    height: u32,
    color_type: ColorType,
    raw: &[u8],
) -> Result<WorkingImage> {
    let data = match color_type {
        ColorType::L8 => gray8_to_rgba8(raw),
        ColorType::La8 => gray_alpha8_to_rgba8(raw),
        ColorType::Rgb8 => rgb8_to_rgba8(raw),
        ColorType::Rgba8 => rgba8_to_rgba8(raw),
        ColorType::L16 => gray16_bytes_to_rgba8(raw),
        ColorType::La16 => gray_alpha16_bytes_to_rgba8(raw),
        ColorType::Rgb16 => rgb16_bytes_to_rgba8(raw),
        ColorType::Rgba16 => rgba16_bytes_to_rgba8(raw),
        other => {
            return Err(Error::unsupported(format!(
                "unsupported decoded color type: {other:?}"
            )));
        }
    };

    Ok(WorkingImage {
        width,
        height,
        data: WorkingData::U8(data),
        color_space: WorkingColorSpace::Source,
    })
}

fn convert_decoded_to_rgba16(
    width: u32,
    height: u32,
    color_type: ColorType,
    raw: &[u8],
) -> Result<WorkingImage> {
    let data = match color_type {
        ColorType::L8 => gray8_to_rgba16(raw),
        ColorType::La8 => gray_alpha8_to_rgba16(raw),
        ColorType::Rgb8 => rgb8_to_rgba16(raw),
        ColorType::Rgba8 => rgba8_to_rgba16(raw),
        ColorType::L16 => gray16_bytes_to_rgba16(raw),
        ColorType::La16 => gray_alpha16_bytes_to_rgba16(raw),
        ColorType::Rgb16 => rgb16_bytes_to_rgba16(raw),
        ColorType::Rgba16 => rgba16_bytes_to_rgba16(raw),
        other => {
            return Err(Error::unsupported(format!(
                "unsupported decoded color type: {other:?}"
            )));
        }
    };

    Ok(WorkingImage {
        width,
        height,
        data: WorkingData::U16(data),
        color_space: WorkingColorSpace::Source,
    })
}

fn gray8_to_rgba8(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() * 4);
    for &l in raw {
        out.extend_from_slice(&[l, l, l, u8::MAX]);
    }
    out
}

fn gray_alpha8_to_rgba8(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() * 2);
    for pixel in raw.chunks_exact(2) {
        out.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
    }
    out
}

fn rgb8_to_rgba8(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() / 3 * 4);
    for pixel in raw.chunks_exact(3) {
        out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], u8::MAX]);
    }
    out
}

fn rgba8_to_rgba8(raw: &[u8]) -> Vec<u8> {
    raw.to_vec()
}

fn gray8_to_rgba16(raw: &[u8]) -> Vec<u16> {
    let mut out = Vec::with_capacity(raw.len() * 4);
    for &l in raw {
        let value = up8(l);
        out.extend_from_slice(&[value, value, value, u16::MAX]);
    }
    out
}

fn gray_alpha8_to_rgba16(raw: &[u8]) -> Vec<u16> {
    let mut out = Vec::with_capacity(raw.len() * 2);
    for pixel in raw.chunks_exact(2) {
        let value = up8(pixel[0]);
        out.extend_from_slice(&[value, value, value, up8(pixel[1])]);
    }
    out
}

fn rgb8_to_rgba16(raw: &[u8]) -> Vec<u16> {
    let mut out = Vec::with_capacity(raw.len() / 3 * 4);
    for pixel in raw.chunks_exact(3) {
        out.extend_from_slice(&[up8(pixel[0]), up8(pixel[1]), up8(pixel[2]), u16::MAX]);
    }
    out
}

fn rgba8_to_rgba16(raw: &[u8]) -> Vec<u16> {
    let mut out = Vec::with_capacity(raw.len());
    for pixel in raw.chunks_exact(4) {
        out.extend_from_slice(&[up8(pixel[0]), up8(pixel[1]), up8(pixel[2]), up8(pixel[3])]);
    }
    out
}

fn gray16_to_rgba16(raw: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(raw.len() * 4);
    for &l in raw {
        out.extend_from_slice(&[l, l, l, u16::MAX]);
    }
    out
}

fn rgb16_to_rgba16(raw: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(raw.len() / 3 * 4);
    for pixel in raw.chunks_exact(3) {
        out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], u16::MAX]);
    }
    out
}

fn rgba16_to_rgba16(raw: &[u16]) -> Vec<u16> {
    raw.to_vec()
}

fn gray16_bytes_to_rgba8(raw: &[u8]) -> Vec<u8> {
    let values = bytes_to_u16(raw);
    let mut out = Vec::with_capacity(values.len() * 4);
    for &l in &values {
        let value = down16(l);
        out.extend_from_slice(&[value, value, value, u8::MAX]);
    }
    out
}

fn gray_alpha16_bytes_to_rgba8(raw: &[u8]) -> Vec<u8> {
    let values = bytes_to_u16(raw);
    let mut out = Vec::with_capacity(values.len() * 2);
    for pixel in values.chunks_exact(2) {
        let value = down16(pixel[0]);
        out.extend_from_slice(&[value, value, value, down16(pixel[1])]);
    }
    out
}

fn rgb16_bytes_to_rgba8(raw: &[u8]) -> Vec<u8> {
    let values = bytes_to_u16(raw);
    let mut out = Vec::with_capacity(values.len() / 3 * 4);
    for pixel in values.chunks_exact(3) {
        out.extend_from_slice(&[
            down16(pixel[0]),
            down16(pixel[1]),
            down16(pixel[2]),
            u8::MAX,
        ]);
    }
    out
}

fn rgba16_bytes_to_rgba8(raw: &[u8]) -> Vec<u8> {
    let values = bytes_to_u16(raw);
    let mut out = Vec::with_capacity(values.len());
    for pixel in values.chunks_exact(4) {
        out.extend_from_slice(&[
            down16(pixel[0]),
            down16(pixel[1]),
            down16(pixel[2]),
            down16(pixel[3]),
        ]);
    }
    out
}

fn gray16_bytes_to_rgba16(raw: &[u8]) -> Vec<u16> {
    let values = bytes_to_u16(raw);
    gray16_to_rgba16(&values)
}

fn gray_alpha16_bytes_to_rgba16(raw: &[u8]) -> Vec<u16> {
    let values = bytes_to_u16(raw);
    let mut out = Vec::with_capacity(values.len() * 2);
    for pixel in values.chunks_exact(2) {
        out.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
    }
    out
}

fn rgb16_bytes_to_rgba16(raw: &[u8]) -> Vec<u16> {
    let values = bytes_to_u16(raw);
    rgb16_to_rgba16(&values)
}

fn rgba16_bytes_to_rgba16(raw: &[u8]) -> Vec<u16> {
    bytes_to_u16(raw)
}

fn bytes_to_u16(raw: &[u8]) -> Vec<u16> {
    raw.chunks_exact(2)
        .map(|chunk| u16::from_ne_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn up8(value: u8) -> u16 {
    (value as u16) * 257
}

fn down16(value: u16) -> u8 {
    ((value as u32 * 255 + 32767) / 65535) as u8
}

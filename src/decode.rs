use std::io::Cursor;

use image::codecs::jpeg::JpegDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::metadata::Orientation;
use image::{ColorType, DynamicImage, ImageBuffer, ImageDecoder, ImageFormat, Rgb, Rgba};
use jpegli::{DecodedImage, DecodedImage16, Decoder, DecoderConfig, PixelLayout};

use crate::error::{Error, Result};
use crate::metadata::SourceMetadata;

#[derive(Debug, Clone)]
pub enum WorkingData {
    Rgb8(Vec<u8>),
    Rgba8(Vec<u8>),
    Rgb16(Vec<u16>),
    Rgba16(Vec<u16>),
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
    pub fn new(
        width: u32,
        height: u32,
        data: WorkingData,
        color_space: WorkingColorSpace,
    ) -> Result<Self> {
        let expected_len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(data.channels()))
            .ok_or_else(|| Error::decode("working image dimensions overflow address space"))?;
        if data.len() != expected_len {
            return Err(Error::decode(format!(
                "invalid working image buffer: expected {expected_len} samples, got {}",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            data,
            color_space,
        })
    }

    pub fn has_alpha(&self) -> bool {
        self.data.has_alpha()
    }

    #[cfg(test)]
    pub fn is_u8(&self) -> bool {
        self.data.is_u8()
    }

    pub fn apply_orientation(self, orientation: Orientation) -> Result<Self> {
        if orientation == Orientation::NoTransforms {
            return Ok(self);
        }

        let color_space = self.color_space;
        match self.data {
            WorkingData::Rgb8(data) => {
                let buffer =
                    ImageBuffer::<Rgb<u8>, Vec<u8>>::from_raw(self.width, self.height, data)
                        .ok_or_else(|| {
                            Error::decode("failed to build RGB8 image for orientation")
                        })?;
                let mut dynamic = DynamicImage::ImageRgb8(buffer);
                dynamic.apply_orientation(orientation);
                let rotated = dynamic.into_rgb8();
                Self::new(
                    rotated.width(),
                    rotated.height(),
                    WorkingData::Rgb8(rotated.into_raw()),
                    color_space,
                )
            }
            WorkingData::Rgba8(data) => {
                let buffer =
                    ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(self.width, self.height, data)
                        .ok_or_else(|| {
                            Error::decode("failed to build RGBA8 image for orientation")
                        })?;
                let mut dynamic = DynamicImage::ImageRgba8(buffer);
                dynamic.apply_orientation(orientation);
                let rotated = dynamic.into_rgba8();
                Self::new(
                    rotated.width(),
                    rotated.height(),
                    WorkingData::Rgba8(rotated.into_raw()),
                    color_space,
                )
            }
            WorkingData::Rgb16(data) => {
                let buffer =
                    ImageBuffer::<Rgb<u16>, Vec<u16>>::from_raw(self.width, self.height, data)
                        .ok_or_else(|| {
                            Error::decode("failed to build RGB16 image for orientation")
                        })?;
                let mut dynamic = DynamicImage::ImageRgb16(buffer);
                dynamic.apply_orientation(orientation);
                let rotated = dynamic.into_rgb16();
                Self::new(
                    rotated.width(),
                    rotated.height(),
                    WorkingData::Rgb16(rotated.into_raw()),
                    color_space,
                )
            }
            WorkingData::Rgba16(data) => {
                let buffer =
                    ImageBuffer::<Rgba<u16>, Vec<u16>>::from_raw(self.width, self.height, data)
                        .ok_or_else(|| {
                            Error::decode("failed to build RGBA16 image for orientation")
                        })?;
                let mut dynamic = DynamicImage::ImageRgba16(buffer);
                dynamic.apply_orientation(orientation);
                let rotated = dynamic.into_rgba16();
                Self::new(
                    rotated.width(),
                    rotated.height(),
                    WorkingData::Rgba16(rotated.into_raw()),
                    color_space,
                )
            }
        }
    }
}

impl WorkingData {
    pub fn channels(&self) -> usize {
        match self {
            Self::Rgb8(_) | Self::Rgb16(_) => 3,
            Self::Rgba8(_) | Self::Rgba16(_) => 4,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Rgb8(data) | Self::Rgba8(data) => data.len(),
            Self::Rgb16(data) | Self::Rgba16(data) => data.len(),
        }
    }

    pub fn has_alpha(&self) -> bool {
        matches!(self, Self::Rgba8(_) | Self::Rgba16(_))
    }

    #[cfg(test)]
    pub fn is_u8(&self) -> bool {
        matches!(self, Self::Rgb8(_) | Self::Rgba8(_))
    }
}

pub fn decode(bytes: &[u8], fast: bool) -> Result<DecodedInput> {
    let format = image::guess_format(bytes).map_err(Error::from)?;

    match format {
        ImageFormat::Jpeg => decode_jpeg(bytes, fast),
        ImageFormat::Png => decode_png(bytes, fast),
        ImageFormat::WebP => decode_webp(bytes, fast),
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
    let channels = match decoded.format {
        PixelLayout::Gray => 1,
        PixelLayout::Rgb => 3,
        PixelLayout::Rgba => 4,
    };
    let samples = normalize_rows(
        decoded.data,
        decoded.width,
        decoded.height,
        channels,
        decoded.stride,
    )?;
    let data = match decoded.format {
        PixelLayout::Gray => WorkingData::Rgb8(gray8_to_rgb8(&samples)),
        PixelLayout::Rgb => WorkingData::Rgb8(samples),
        PixelLayout::Rgba => rgba8_to_working_data(samples),
    };

    WorkingImage::new(
        decoded.width,
        decoded.height,
        data,
        WorkingColorSpace::Source,
    )
}

fn working_image_from_jpeg_u16(decoded: DecodedImage16) -> Result<WorkingImage> {
    let channels = match decoded.format {
        PixelLayout::Gray => 1,
        PixelLayout::Rgb => 3,
        PixelLayout::Rgba => 4,
    };
    let samples = normalize_rows(
        decoded.data,
        decoded.width,
        decoded.height,
        channels,
        decoded.stride,
    )?;
    let data = match decoded.format {
        PixelLayout::Gray => WorkingData::Rgb16(gray16_to_rgb16(&samples)),
        PixelLayout::Rgb => WorkingData::Rgb16(samples),
        PixelLayout::Rgba => rgba16_to_working_data(samples),
    };

    WorkingImage::new(
        decoded.width,
        decoded.height,
        data,
        WorkingColorSpace::Source,
    )
}

fn working_image_from_decoder<D: ImageDecoder>(decoder: D, fast: bool) -> Result<WorkingImage> {
    let (width, height) = decoder.dimensions();
    let color_type = decoder.color_type();
    let mut buf = vec![0u8; decoder.total_bytes() as usize];
    decoder.read_image(&mut buf)?;

    let data = if fast {
        convert_decoded_to_8bit(color_type, buf)?
    } else {
        convert_decoded_to_16bit(color_type, buf)?
    };
    WorkingImage::new(width, height, data, WorkingColorSpace::Source)
}

fn convert_decoded_to_8bit(color_type: ColorType, raw: Vec<u8>) -> Result<WorkingData> {
    let data = match color_type {
        ColorType::L8 => WorkingData::Rgb8(gray8_to_rgb8(&raw)),
        ColorType::La8 => gray_alpha8_to_working_data(&raw),
        ColorType::Rgb8 => WorkingData::Rgb8(raw),
        ColorType::Rgba8 => rgba8_to_working_data(raw),
        ColorType::L16 => WorkingData::Rgb8(gray16_bytes_to_rgb8(&raw)),
        ColorType::La16 => gray_alpha16_bytes_to_working_data8(&raw),
        ColorType::Rgb16 => WorkingData::Rgb8(rgb16_bytes_to_rgb8(&raw)),
        ColorType::Rgba16 => rgba16_bytes_to_working_data8(&raw),
        other => {
            return Err(Error::unsupported(format!(
                "unsupported decoded color type: {other:?}"
            )));
        }
    };
    Ok(data)
}

fn convert_decoded_to_16bit(color_type: ColorType, raw: Vec<u8>) -> Result<WorkingData> {
    let data = match color_type {
        ColorType::L8 => WorkingData::Rgb16(gray8_to_rgb16(&raw)),
        ColorType::La8 => gray_alpha8_to_working_data16(&raw),
        ColorType::Rgb8 => WorkingData::Rgb16(rgb8_to_rgb16(&raw)),
        ColorType::Rgba8 => rgba8_to_working_data16(&raw),
        ColorType::L16 => WorkingData::Rgb16(gray16_bytes_to_rgb16(&raw)),
        ColorType::La16 => gray_alpha16_bytes_to_working_data16(&raw),
        ColorType::Rgb16 => WorkingData::Rgb16(rgb16_bytes_to_rgb16(&raw)),
        ColorType::Rgba16 => rgba16_bytes_to_working_data16(&raw),
        other => {
            return Err(Error::unsupported(format!(
                "unsupported decoded color type: {other:?}"
            )));
        }
    };
    Ok(data)
}

fn normalize_rows<T: Copy>(
    mut data: Vec<T>,
    width: u32,
    height: u32,
    channels: usize,
    stride: usize,
) -> Result<Vec<T>> {
    let row_len = (width as usize)
        .checked_mul(channels)
        .ok_or_else(|| Error::decode("decoded row length is too large"))?;
    let packed_len = row_len
        .checked_mul(height as usize)
        .ok_or_else(|| Error::decode("decoded image buffer is too large"))?;
    let required = stride
        .checked_mul(height as usize)
        .ok_or_else(|| Error::decode("decoded row layout is too large"))?;
    if stride < row_len || data.len() < required {
        return Err(Error::decode("decoder returned an invalid row layout"));
    }
    if stride == row_len {
        data.truncate(packed_len);
        return Ok(data);
    }

    let mut packed = Vec::with_capacity(packed_len);
    for row in data.chunks(stride).take(height as usize) {
        packed.extend_from_slice(&row[..row_len]);
    }
    Ok(packed)
}

fn gray8_to_rgb8(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() * 3);
    for &l in raw {
        out.extend_from_slice(&[l, l, l]);
    }
    out
}

fn gray_alpha8_to_working_data(raw: &[u8]) -> WorkingData {
    if raw.chunks_exact(2).all(|pixel| pixel[1] == u8::MAX) {
        let mut out = Vec::with_capacity(raw.len() / 2 * 3);
        for pixel in raw.chunks_exact(2) {
            out.extend_from_slice(&[pixel[0], pixel[0], pixel[0]]);
        }
        return WorkingData::Rgb8(out);
    }

    let mut out = Vec::with_capacity(raw.len() * 2);
    for pixel in raw.chunks_exact(2) {
        out.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
    }
    WorkingData::Rgba8(out)
}

fn rgba8_to_working_data(raw: Vec<u8>) -> WorkingData {
    if raw.chunks_exact(4).all(|pixel| pixel[3] == u8::MAX) {
        let mut rgb = Vec::with_capacity(raw.len() / 4 * 3);
        for pixel in raw.chunks_exact(4) {
            rgb.extend_from_slice(&pixel[..3]);
        }
        WorkingData::Rgb8(rgb)
    } else {
        WorkingData::Rgba8(raw)
    }
}

fn gray8_to_rgb16(raw: &[u8]) -> Vec<u16> {
    let mut out = Vec::with_capacity(raw.len() * 3);
    for &l in raw {
        let value = up8(l);
        out.extend_from_slice(&[value, value, value]);
    }
    out
}

fn gray_alpha8_to_working_data16(raw: &[u8]) -> WorkingData {
    if raw.chunks_exact(2).all(|pixel| pixel[1] == u8::MAX) {
        let mut out = Vec::with_capacity(raw.len() / 2 * 3);
        for pixel in raw.chunks_exact(2) {
            let value = up8(pixel[0]);
            out.extend_from_slice(&[value, value, value]);
        }
        return WorkingData::Rgb16(out);
    }

    let mut out = Vec::with_capacity(raw.len() * 2);
    for pixel in raw.chunks_exact(2) {
        let value = up8(pixel[0]);
        out.extend_from_slice(&[value, value, value, up8(pixel[1])]);
    }
    WorkingData::Rgba16(out)
}

fn rgb8_to_rgb16(raw: &[u8]) -> Vec<u16> {
    raw.iter().copied().map(up8).collect()
}

fn rgba8_to_working_data16(raw: &[u8]) -> WorkingData {
    if raw.chunks_exact(4).all(|pixel| pixel[3] == u8::MAX) {
        let mut rgb = Vec::with_capacity(raw.len() / 4 * 3);
        for pixel in raw.chunks_exact(4) {
            rgb.extend(pixel[..3].iter().copied().map(up8));
        }
        return WorkingData::Rgb16(rgb);
    }

    WorkingData::Rgba16(raw.iter().copied().map(up8).collect())
}

fn gray16_to_rgb16(raw: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(raw.len() * 3);
    for &l in raw {
        out.extend_from_slice(&[l, l, l]);
    }
    out
}

fn rgba16_to_working_data(raw: Vec<u16>) -> WorkingData {
    if raw.chunks_exact(4).all(|pixel| pixel[3] == u16::MAX) {
        let mut rgb = Vec::with_capacity(raw.len() / 4 * 3);
        for pixel in raw.chunks_exact(4) {
            rgb.extend_from_slice(&pixel[..3]);
        }
        WorkingData::Rgb16(rgb)
    } else {
        WorkingData::Rgba16(raw)
    }
}

fn gray16_bytes_to_rgb8(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() / 2 * 3);
    for sample in raw.chunks_exact(2) {
        let value = down16(read_u16(sample));
        out.extend_from_slice(&[value, value, value]);
    }
    out
}

fn gray_alpha16_bytes_to_working_data8(raw: &[u8]) -> WorkingData {
    let opaque = raw
        .chunks_exact(4)
        .all(|pixel| read_u16(&pixel[2..]) == u16::MAX);
    let channels = if opaque { 3 } else { 4 };
    let mut out = Vec::with_capacity(raw.len() / 4 * channels);
    for pixel in raw.chunks_exact(4) {
        let value = down16(read_u16(pixel));
        out.extend_from_slice(&[value, value, value]);
        if !opaque {
            out.push(down16(read_u16(&pixel[2..])));
        }
    }
    if opaque {
        WorkingData::Rgb8(out)
    } else {
        WorkingData::Rgba8(out)
    }
}

fn rgb16_bytes_to_rgb8(raw: &[u8]) -> Vec<u8> {
    raw.chunks_exact(2)
        .map(|sample| down16(read_u16(sample)))
        .collect()
}

fn rgba16_bytes_to_working_data8(raw: &[u8]) -> WorkingData {
    let opaque = raw
        .chunks_exact(8)
        .all(|pixel| read_u16(&pixel[6..]) == u16::MAX);
    let channels = if opaque { 3 } else { 4 };
    let mut out = Vec::with_capacity(raw.len() / 8 * channels);
    for pixel in raw.chunks_exact(8) {
        out.push(down16(read_u16(pixel)));
        out.push(down16(read_u16(&pixel[2..])));
        out.push(down16(read_u16(&pixel[4..])));
        if !opaque {
            out.push(down16(read_u16(&pixel[6..])));
        }
    }
    if opaque {
        WorkingData::Rgb8(out)
    } else {
        WorkingData::Rgba8(out)
    }
}

fn gray16_bytes_to_rgb16(raw: &[u8]) -> Vec<u16> {
    let mut out = Vec::with_capacity(raw.len() / 2 * 3);
    for sample in raw.chunks_exact(2) {
        let value = read_u16(sample);
        out.extend_from_slice(&[value, value, value]);
    }
    out
}

fn gray_alpha16_bytes_to_working_data16(raw: &[u8]) -> WorkingData {
    let opaque = raw
        .chunks_exact(4)
        .all(|pixel| read_u16(&pixel[2..]) == u16::MAX);
    let channels = if opaque { 3 } else { 4 };
    let mut out = Vec::with_capacity(raw.len() / 4 * channels);
    for pixel in raw.chunks_exact(4) {
        let value = read_u16(pixel);
        out.extend_from_slice(&[value, value, value]);
        if !opaque {
            out.push(read_u16(&pixel[2..]));
        }
    }
    if opaque {
        WorkingData::Rgb16(out)
    } else {
        WorkingData::Rgba16(out)
    }
}

fn rgb16_bytes_to_rgb16(raw: &[u8]) -> Vec<u16> {
    raw.chunks_exact(2).map(read_u16).collect()
}

fn rgba16_bytes_to_working_data16(raw: &[u8]) -> WorkingData {
    let opaque = raw
        .chunks_exact(8)
        .all(|pixel| read_u16(&pixel[6..]) == u16::MAX);
    let channels = if opaque { 3 } else { 4 };
    let mut out = Vec::with_capacity(raw.len() / 8 * channels);
    for pixel in raw.chunks_exact(8) {
        out.push(read_u16(pixel));
        out.push(read_u16(&pixel[2..]));
        out.push(read_u16(&pixel[4..]));
        if !opaque {
            out.push(read_u16(&pixel[6..]));
        }
    }
    if opaque {
        WorkingData::Rgb16(out)
    } else {
        WorkingData::Rgba16(out)
    }
}

fn read_u16(raw: &[u8]) -> u16 {
    u16::from_ne_bytes([raw[0], raw[1]])
}

fn up8(value: u8) -> u16 {
    (value as u16) * 257
}

fn down16(value: u16) -> u8 {
    ((value as u32 * 255 + 32767) / 65535) as u8
}

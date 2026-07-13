use bytemuck::cast_slice_mut;
use lcms2::{
    ColorSpaceSignature, DisallowCache, Flags, Intent, PixelFormat, Profile, Tag, TagSignature,
    ThreadContext, ToneCurve, Transform, XYZ2xyY,
};
use rayon::prelude::*;

use crate::decode::WorkingColorSpace;
use crate::decode::{WorkingData, WorkingImage};
use crate::error::{Error, Result};

pub struct ColorPipeline {
    source_profile: Profile,
    linear_profile: Profile,
    source_icc: Option<Vec<u8>>,
}

pub struct SimpleYuvTables {
    pub source_u8_to_linear: [Vec<u16>; 3],
    pub linear_to_source_u8: [Vec<u16>; 3],
    pub source_u16_to_linear: [Vec<u16>; 3],
    pub linear_to_source_u16: [Vec<u16>; 3],
}

#[cfg(test)]
pub struct LinearU16Tables {
    pub source_to_linear: [Vec<u16>; 3],
    pub linear_to_source: [Vec<u16>; 3],
    pub chroma_source_to_linear: [Vec<u16>; 3],
    pub chroma_linear_to_source: [Vec<u16>; 3],
}

struct SimpleYuvChannelTables {
    source_u8_to_linear: Vec<u16>,
    linear_to_source_u8: Vec<u16>,
    source_u16_to_linear: Vec<u16>,
    linear_to_source_u16: Vec<u16>,
}

pub fn validate_source_profile(icc: Option<&[u8]>, require_matrix_shaper: bool) -> Result<()> {
    let Some(source_profile) = load_source_profile(icc)? else {
        return Ok(());
    };

    if require_matrix_shaper && !source_profile.is_matrix_shaper() {
        return Err(Error::unsupported(
            "only matrix-shaper RGB ICC profiles are supported for the linear RGB pipeline in v1",
        ));
    }

    Ok(())
}

impl ColorPipeline {
    pub fn new(icc: Option<&[u8]>) -> Result<Self> {
        let source_profile = load_source_profile(icc)?.unwrap_or_else(Profile::new_srgb);
        if !source_profile.is_matrix_shaper() {
            return Err(Error::unsupported(
                "only matrix-shaper RGB ICC profiles are supported for the linear RGB pipeline in v1",
            ));
        }

        let linear_profile = make_linear_profile(&source_profile)?;

        Ok(Self {
            source_profile,
            linear_profile,
            source_icc: icc.map(<[u8]>::to_vec),
        })
    }

    pub fn simpleyuv_tables(&self) -> Result<SimpleYuvTables> {
        let red = build_simpleyuv_channel_tables(read_tone_curve(
            &self.source_profile,
            TagSignature::RedTRCTag,
        )?);
        let green = build_simpleyuv_channel_tables(read_tone_curve(
            &self.source_profile,
            TagSignature::GreenTRCTag,
        )?);
        let blue = build_simpleyuv_channel_tables(read_tone_curve(
            &self.source_profile,
            TagSignature::BlueTRCTag,
        )?);

        Ok(SimpleYuvTables {
            source_u8_to_linear: [
                red.source_u8_to_linear,
                green.source_u8_to_linear,
                blue.source_u8_to_linear,
            ],
            linear_to_source_u8: [
                red.linear_to_source_u8,
                green.linear_to_source_u8,
                blue.linear_to_source_u8,
            ],
            source_u16_to_linear: [
                red.source_u16_to_linear,
                green.source_u16_to_linear,
                blue.source_u16_to_linear,
            ],
            linear_to_source_u16: [
                red.linear_to_source_u16,
                green.linear_to_source_u16,
                blue.linear_to_source_u16,
            ],
        })
    }

    #[cfg(test)]
    pub fn linear_u16_tables(&self) -> Result<LinearU16Tables> {
        let red = build_u16_subsampling_channel_tables(read_tone_curve(
            &self.source_profile,
            TagSignature::RedTRCTag,
        )?);
        let green = build_u16_subsampling_channel_tables(read_tone_curve(
            &self.source_profile,
            TagSignature::GreenTRCTag,
        )?);
        let blue = build_u16_subsampling_channel_tables(read_tone_curve(
            &self.source_profile,
            TagSignature::BlueTRCTag,
        )?);

        Ok(LinearU16Tables {
            source_to_linear: build_rgb16_transform_luts(
                &self.source_profile,
                &self.linear_profile,
                "source->linear",
            )?,
            linear_to_source: build_rgb16_transform_luts(
                &self.linear_profile,
                &self.source_profile,
                "linear->source",
            )?,
            chroma_source_to_linear: [red.0, green.0, blue.0],
            chroma_linear_to_source: [red.1, green.1, blue.1],
        })
    }

    #[cfg(test)]
    pub fn to_linear_u16_lut_in_place(
        &self,
        image: &mut WorkingImage,
        tables: &LinearU16Tables,
    ) -> Result<()> {
        if image.color_space == WorkingColorSpace::LinearRgb {
            return Ok(());
        }

        let (data, channels) = match &mut image.data {
            WorkingData::Rgb16(data) => (data, 3),
            WorkingData::Rgba16(data) => (data, 4),
            WorkingData::Rgb8(_) | WorkingData::Rgba8(_) => {
                return Err(Error::color(
                    "--fasthq requires the high-bit-depth decode path",
                ));
            }
        };

        data.par_chunks_exact_mut(channels).for_each(|pixel| {
            pixel[0] = tables.source_to_linear[0][pixel[0] as usize];
            pixel[1] = tables.source_to_linear[1][pixel[1] as usize];
            pixel[2] = tables.source_to_linear[2][pixel[2] as usize];
        });
        image.color_space = WorkingColorSpace::LinearRgb;
        Ok(())
    }

    pub fn to_linear_in_place(&self, image: &mut WorkingImage) -> Result<()> {
        if image.color_space == WorkingColorSpace::LinearRgb {
            return Ok(());
        }

        match &mut image.data {
            WorkingData::Rgb8(rgb8) => {
                let transform = Transform::<[u8; 3], [u8; 3]>::new(
                    &self.source_profile,
                    PixelFormat::RGB_8,
                    &self.linear_profile,
                    PixelFormat::RGB_8,
                    Intent::Perceptual,
                )
                .map_err(|err| {
                    Error::color(format!("failed to build source->linear transform: {err}"))
                })?;
                transform.transform_in_place(cast_slice_mut::<u8, [u8; 3]>(rgb8));
            }
            WorkingData::Rgba8(rgba8) => {
                let transform = Transform::<[u8; 4], [u8; 4]>::new_flags(
                    &self.source_profile,
                    PixelFormat::RGBA_8,
                    &self.linear_profile,
                    PixelFormat::RGBA_8,
                    Intent::Perceptual,
                    Flags::COPY_ALPHA,
                )
                .map_err(|err| {
                    Error::color(format!("failed to build source->linear transform: {err}"))
                })?;
                let pixels = cast_slice_mut::<u8, [u8; 4]>(rgba8);
                transform.transform_in_place(pixels);
            }
            WorkingData::Rgb16(rgb16) => {
                let transform = Transform::<[u16; 3], [u16; 3]>::new(
                    &self.source_profile,
                    PixelFormat::RGB_16,
                    &self.linear_profile,
                    PixelFormat::RGB_16,
                    Intent::Perceptual,
                )
                .map_err(|err| {
                    Error::color(format!("failed to build source->linear transform: {err}"))
                })?;
                transform.transform_in_place(cast_slice_mut::<u16, [u16; 3]>(rgb16));
            }
            WorkingData::Rgba16(rgba16) => {
                let transform = Transform::<[u16; 4], [u16; 4]>::new_flags(
                    &self.source_profile,
                    PixelFormat::RGBA_16,
                    &self.linear_profile,
                    PixelFormat::RGBA_16,
                    Intent::Perceptual,
                    Flags::COPY_ALPHA,
                )
                .map_err(|err| {
                    Error::color(format!("failed to build source->linear transform: {err}"))
                })?;
                let pixels = cast_slice_mut::<u16, [u16; 4]>(rgba16);
                transform.transform_in_place(pixels);
            }
        }

        image.color_space = WorkingColorSpace::LinearRgb;
        Ok(())
    }

    pub fn to_linear_parallel_in_place(&self, image: &mut WorkingImage) -> Result<()> {
        if image.color_space == WorkingColorSpace::LinearRgb {
            return Ok(());
        }

        match &mut image.data {
            WorkingData::Rgb16(data) => {
                transform_rgb16_parallel(self.source_icc.as_deref(), data, false)?
            }
            WorkingData::Rgba16(data) => {
                transform_rgba16_parallel(self.source_icc.as_deref(), data, false)?
            }
            WorkingData::Rgb8(_) | WorkingData::Rgba8(_) => {
                return self.to_linear_in_place(image);
            }
        }

        image.color_space = WorkingColorSpace::LinearRgb;
        Ok(())
    }

    pub fn convert_from_linear_in_place(&self, image: &mut WorkingImage) -> Result<()> {
        if image.color_space == WorkingColorSpace::Source {
            return Ok(());
        }

        match &mut image.data {
            WorkingData::Rgb8(rgb8) => {
                let transform = Transform::<[u8; 3], [u8; 3]>::new(
                    &self.linear_profile,
                    PixelFormat::RGB_8,
                    &self.source_profile,
                    PixelFormat::RGB_8,
                    Intent::Perceptual,
                )
                .map_err(|err| {
                    Error::color(format!("failed to build linear->source transform: {err}"))
                })?;
                transform.transform_in_place(cast_slice_mut::<u8, [u8; 3]>(rgb8));
            }
            WorkingData::Rgba8(rgba8) => {
                let transform = Transform::<[u8; 4], [u8; 4]>::new_flags(
                    &self.linear_profile,
                    PixelFormat::RGBA_8,
                    &self.source_profile,
                    PixelFormat::RGBA_8,
                    Intent::Perceptual,
                    Flags::COPY_ALPHA,
                )
                .map_err(|err| {
                    Error::color(format!("failed to build linear->source transform: {err}"))
                })?;
                let pixels = cast_slice_mut::<u8, [u8; 4]>(rgba8);
                transform.transform_in_place(pixels);
            }
            WorkingData::Rgb16(rgb16) => {
                let transform = Transform::<[u16; 3], [u16; 3]>::new(
                    &self.linear_profile,
                    PixelFormat::RGB_16,
                    &self.source_profile,
                    PixelFormat::RGB_16,
                    Intent::Perceptual,
                )
                .map_err(|err| {
                    Error::color(format!("failed to build linear->source transform: {err}"))
                })?;
                transform.transform_in_place(cast_slice_mut::<u16, [u16; 3]>(rgb16));
            }
            WorkingData::Rgba16(rgba16) => {
                let transform = Transform::<[u16; 4], [u16; 4]>::new_flags(
                    &self.linear_profile,
                    PixelFormat::RGBA_16,
                    &self.source_profile,
                    PixelFormat::RGBA_16,
                    Intent::Perceptual,
                    Flags::COPY_ALPHA,
                )
                .map_err(|err| {
                    Error::color(format!("failed to build linear->source transform: {err}"))
                })?;
                let pixels = cast_slice_mut::<u16, [u16; 4]>(rgba16);
                transform.transform_in_place(pixels);
            }
        }

        image.color_space = WorkingColorSpace::Source;
        Ok(())
    }

    pub fn convert_from_linear_parallel_in_place(&self, image: &mut WorkingImage) -> Result<()> {
        if image.color_space == WorkingColorSpace::Source {
            return Ok(());
        }

        match &mut image.data {
            WorkingData::Rgb16(data) => {
                transform_rgb16_parallel(self.source_icc.as_deref(), data, true)?
            }
            WorkingData::Rgba16(data) => {
                transform_rgba16_parallel(self.source_icc.as_deref(), data, true)?
            }
            WorkingData::Rgb8(_) | WorkingData::Rgba8(_) => {
                return self.convert_from_linear_in_place(image);
            }
        }

        image.color_space = WorkingColorSpace::Source;
        Ok(())
    }
}

fn transform_rgb16_parallel(
    source_icc: Option<&[u8]>,
    data: &mut [u16],
    reverse: bool,
) -> Result<()> {
    let context = ThreadContext::new();
    let (source, linear) = thread_profiles(&context, source_icc)?;
    let (input, output, direction) = if reverse {
        (&linear, &source, "linear->source")
    } else {
        (&source, &linear, "source->linear")
    };
    let transform =
        Transform::<[u16; 3], [u16; 3], ThreadContext, DisallowCache>::new_flags_context(
            &context,
            input,
            PixelFormat::RGB_16,
            output,
            PixelFormat::RGB_16,
            Intent::Perceptual,
            Flags::NO_CACHE,
        )
        .map_err(|err| Error::color(format!("failed to build {direction} transform: {err}")))?;
    cast_slice_mut::<u16, [u16; 3]>(data)
        .par_chunks_mut(256 * 1024)
        .for_each(|pixels| transform.transform_in_place(pixels));
    Ok(())
}

fn transform_rgba16_parallel(
    source_icc: Option<&[u8]>,
    data: &mut [u16],
    reverse: bool,
) -> Result<()> {
    let context = ThreadContext::new();
    let (source, linear) = thread_profiles(&context, source_icc)?;
    let (input, output, direction) = if reverse {
        (&linear, &source, "linear->source")
    } else {
        (&source, &linear, "source->linear")
    };
    let transform =
        Transform::<[u16; 4], [u16; 4], ThreadContext, DisallowCache>::new_flags_context(
            &context,
            input,
            PixelFormat::RGBA_16,
            output,
            PixelFormat::RGBA_16,
            Intent::Perceptual,
            Flags::NO_CACHE | Flags::COPY_ALPHA,
        )
        .map_err(|err| Error::color(format!("failed to build {direction} transform: {err}")))?;
    cast_slice_mut::<u16, [u16; 4]>(data)
        .par_chunks_mut(256 * 1024)
        .for_each(|pixels| transform.transform_in_place(pixels));
    Ok(())
}

fn thread_profiles(
    context: &ThreadContext,
    source_icc: Option<&[u8]>,
) -> Result<(Profile<ThreadContext>, Profile<ThreadContext>)> {
    let source = match source_icc {
        Some(icc) => Profile::<ThreadContext>::new_icc_context(context, icc)
            .map_err(|err| Error::color(format!("failed to load thread ICC profile: {err}")))?,
        None => Profile::<ThreadContext>::new_srgb_context(context),
    };
    let linear = make_thread_linear_profile(context, &source)?;
    Ok((source, linear))
}

fn make_thread_linear_profile(
    context: &ThreadContext,
    source_profile: &Profile<ThreadContext>,
) -> Result<Profile<ThreadContext>> {
    let white_point = match source_profile.read_tag(TagSignature::MediaWhitePointTag) {
        Tag::CIEXYZ(xyz) => XYZ2xyY(xyz),
        _ => {
            return Err(Error::color(
                "ICC profile is missing MediaWhitePointTag for the linear RGB pipeline",
            ));
        }
    };
    let colorant = |tag, name| match source_profile.read_tag(tag) {
        Tag::CIEXYZ(xyz) => Ok(XYZ2xyY(xyz)),
        _ => Err(Error::color(format!("ICC profile is missing {name}"))),
    };
    let primaries = lcms2::CIExyYTRIPLE {
        Red: colorant(TagSignature::RedColorantTag, "RedColorantTag")?,
        Green: colorant(TagSignature::GreenColorantTag, "GreenColorantTag")?,
        Blue: colorant(TagSignature::BlueColorantTag, "BlueColorantTag")?,
    };
    let linear = ToneCurve::new(1.0);
    Profile::<ThreadContext>::new_rgb_context(
        context,
        &white_point,
        &primaries,
        &[&linear, &linear, &linear],
    )
    .map_err(|err| Error::color(format!("failed to build thread linear RGB profile: {err}")))
}

fn load_source_profile(icc: Option<&[u8]>) -> Result<Option<Profile>> {
    let Some(bytes) = icc else {
        return Ok(None);
    };

    let source_profile = Profile::new_icc(bytes)
        .map_err(|err| Error::color(format!("failed to parse ICC profile: {err}")))?;
    if source_profile.color_space() != ColorSpaceSignature::RgbData {
        return Err(Error::unsupported(
            "only RGB ICC profiles are supported in v1",
        ));
    }

    Ok(Some(source_profile))
}

fn make_linear_profile(source_profile: &Profile) -> Result<Profile> {
    let white_point = match source_profile.read_tag(TagSignature::MediaWhitePointTag) {
        Tag::CIEXYZ(xyz) => XYZ2xyY(xyz),
        _ => {
            return Err(Error::color(
                "ICC profile is missing MediaWhitePointTag for the linear RGB pipeline",
            ));
        }
    };

    let red = match source_profile.read_tag(TagSignature::RedColorantTag) {
        Tag::CIEXYZ(xyz) => XYZ2xyY(xyz),
        _ => return Err(Error::color("ICC profile is missing RedColorantTag")),
    };
    let green = match source_profile.read_tag(TagSignature::GreenColorantTag) {
        Tag::CIEXYZ(xyz) => XYZ2xyY(xyz),
        _ => return Err(Error::color("ICC profile is missing GreenColorantTag")),
    };
    let blue = match source_profile.read_tag(TagSignature::BlueColorantTag) {
        Tag::CIEXYZ(xyz) => XYZ2xyY(xyz),
        _ => return Err(Error::color("ICC profile is missing BlueColorantTag")),
    };

    let primaries = lcms2::CIExyYTRIPLE {
        Red: red,
        Green: green,
        Blue: blue,
    };

    let linear = ToneCurve::new(1.0);
    Profile::new_rgb(&white_point, &primaries, &[&linear, &linear, &linear])
        .map_err(|err| Error::color(format!("failed to build linear RGB profile: {err}")))
}

fn read_tone_curve(profile: &Profile, tag: TagSignature) -> Result<&lcms2::ToneCurveRef> {
    match profile.read_tag(tag) {
        Tag::ToneCurve(curve) => Ok(curve),
        _ => Err(Error::color(format!("ICC profile is missing {tag:?}"))),
    }
}

fn build_simpleyuv_channel_tables(curve: &lcms2::ToneCurveRef) -> SimpleYuvChannelTables {
    let inverse = curve.reversed_samples(65_536);
    SimpleYuvChannelTables {
        source_u8_to_linear: build_source_to_linear_lut(curve, 10),
        linear_to_source_u8: build_linear_to_source_lut(&inverse, 10),
        source_u16_to_linear: build_source_to_linear_lut(curve, 14),
        linear_to_source_u16: build_linear_to_source_lut(&inverse, 14),
    }
}

#[cfg(test)]
fn build_u16_subsampling_channel_tables(curve: &lcms2::ToneCurveRef) -> (Vec<u16>, Vec<u16>) {
    let inverse = curve.reversed_samples(65_536);
    (
        build_source_to_linear_lut(curve, 14),
        build_linear_to_source_lut(&inverse, 14),
    )
}

#[cfg(test)]
fn build_rgb16_transform_luts(
    input_profile: &Profile,
    output_profile: &Profile,
    direction: &str,
) -> Result<[Vec<u16>; 3]> {
    let transform = Transform::<[u16; 3], [u16; 3]>::new(
        input_profile,
        PixelFormat::RGB_16,
        output_profile,
        PixelFormat::RGB_16,
        Intent::Perceptual,
    )
    .map_err(|err| Error::color(format!("failed to build {direction} LUT transform: {err}")))?;

    let values_per_channel = u16::MAX as usize + 1;
    let mut samples = Vec::with_capacity(values_per_channel * 3);
    for channel in 0..3 {
        for value in 0..=u16::MAX {
            let mut pixel = [0u16; 3];
            pixel[channel] = value;
            samples.push(pixel);
        }
    }
    transform.transform_in_place(&mut samples);

    let mut tables: [Vec<u16>; 3] = std::array::from_fn(|_| vec![0; values_per_channel]);
    for channel in 0..3 {
        let offset = channel * values_per_channel;
        for (value, pixel) in samples[offset..offset + values_per_channel]
            .iter()
            .enumerate()
        {
            tables[channel][value] = pixel[channel];
        }
    }
    Ok(tables)
}

fn build_source_to_linear_lut(curve: &lcms2::ToneCurveRef, internal_bit_depth: u8) -> Vec<u16> {
    let max = (1u32 << internal_bit_depth) - 1;
    (0..=max)
        .map(|code| curve.eval(scale_code_to_u16(code, max)))
        .collect()
}

fn build_linear_to_source_lut(inverse: &lcms2::ToneCurveRef, internal_bit_depth: u8) -> Vec<u16> {
    let max = (1u32 << internal_bit_depth) - 1;
    (0..=u16::MAX)
        .map(|linear| scale_code_from_u16(inverse.eval(linear), max))
        .collect()
}

fn scale_code_to_u16(value: u32, max: u32) -> u16 {
    ((value * u16::MAX as u32 + max / 2) / max) as u16
}

fn scale_code_from_u16(value: u16, max: u32) -> u16 {
    ((value as u32 * max + (u16::MAX as u32 / 2)) / u16::MAX as u32) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u16_luts_match_lcms_transform() {
        let pipeline = ColorPipeline::new(None).unwrap();
        let tables = pipeline.linear_u16_tables().unwrap();
        let samples = (0..4096u32)
            .flat_map(|index| {
                [
                    (index.wrapping_mul(15_485) & 0xffff) as u16,
                    (index.wrapping_mul(27_149).wrapping_add(12_345) & 0xffff) as u16,
                    (index.wrapping_mul(39_307).wrapping_add(54_321) & 0xffff) as u16,
                ]
            })
            .collect::<Vec<_>>();
        let mut lcms = WorkingImage::new(
            4096,
            1,
            WorkingData::Rgb16(samples.clone()),
            WorkingColorSpace::Source,
        )
        .unwrap();
        let mut lut = lcms.clone();
        pipeline.to_linear_in_place(&mut lcms).unwrap();
        pipeline
            .to_linear_u16_lut_in_place(&mut lut, &tables)
            .unwrap();

        let lcms = match lcms.data {
            WorkingData::Rgb16(data) => data,
            _ => unreachable!(),
        };
        let lut = match lut.data {
            WorkingData::Rgb16(data) => data,
            _ => unreachable!(),
        };
        let max_diff = lcms
            .iter()
            .zip(lut)
            .map(|(lcms, lut)| (*lcms as i32 - lut as i32).unsigned_abs())
            .max()
            .unwrap();
        assert_eq!(max_diff, 0);

        let linear_samples = (0..4096u32)
            .flat_map(|index| {
                [
                    (index.wrapping_mul(51_007) & 0xffff) as u16,
                    (index.wrapping_mul(9_973).wrapping_add(22_222) & 0xffff) as u16,
                    (index.wrapping_mul(31_337).wrapping_add(44_444) & 0xffff) as u16,
                ]
            })
            .collect::<Vec<_>>();
        let mut transformed = WorkingImage::new(
            4096,
            1,
            WorkingData::Rgb16(linear_samples.clone()),
            WorkingColorSpace::LinearRgb,
        )
        .unwrap();
        pipeline
            .convert_from_linear_in_place(&mut transformed)
            .unwrap();
        let expected = match transformed.data {
            WorkingData::Rgb16(data) => data,
            _ => unreachable!(),
        };
        let actual = linear_samples
            .chunks_exact(3)
            .flat_map(|pixel| {
                [
                    tables.linear_to_source[0][pixel[0] as usize],
                    tables.linear_to_source[1][pixel[1] as usize],
                    tables.linear_to_source[2][pixel[2] as usize],
                ]
            })
            .collect::<Vec<_>>();
        let max_diff = expected
            .iter()
            .zip(actual)
            .map(|(expected, actual)| (*expected as i32 - actual as i32).unsigned_abs())
            .max()
            .unwrap();
        assert_eq!(max_diff, 0);
    }
}

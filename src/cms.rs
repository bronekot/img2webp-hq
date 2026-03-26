use bytemuck::cast_slice_mut;
use lcms2::{
    ColorSpaceSignature, Flags, Intent, PixelFormat, Profile, Tag, TagSignature, ToneCurve,
    Transform, XYZ2xyY,
};

use crate::decode::WorkingColorSpace;
use crate::decode::{WorkingData, WorkingImage};
use crate::error::{Error, Result};

pub struct ResizeColorPipeline {
    source_profile: Profile,
    linear_profile: Profile,
}

pub struct SimpleYuvTables {
    pub source_u8_to_linear: [Vec<u16>; 3],
    pub linear_to_source_u8: [Vec<u16>; 3],
    pub source_u16_to_linear: [Vec<u16>; 3],
    pub linear_to_source_u16: [Vec<u16>; 3],
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

impl ResizeColorPipeline {
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
        })
    }

    pub fn simpleyuv_tables(&self) -> Result<SimpleYuvTables> {
        Ok(SimpleYuvTables {
            source_u8_to_linear: [
                build_source_to_linear_lut(
                    read_tone_curve(&self.source_profile, TagSignature::RedTRCTag)?,
                    10,
                ),
                build_source_to_linear_lut(
                    read_tone_curve(&self.source_profile, TagSignature::GreenTRCTag)?,
                    10,
                ),
                build_source_to_linear_lut(
                    read_tone_curve(&self.source_profile, TagSignature::BlueTRCTag)?,
                    10,
                ),
            ],
            linear_to_source_u8: [
                build_linear_to_source_lut(
                    read_tone_curve(&self.source_profile, TagSignature::RedTRCTag)?,
                    10,
                ),
                build_linear_to_source_lut(
                    read_tone_curve(&self.source_profile, TagSignature::GreenTRCTag)?,
                    10,
                ),
                build_linear_to_source_lut(
                    read_tone_curve(&self.source_profile, TagSignature::BlueTRCTag)?,
                    10,
                ),
            ],
            source_u16_to_linear: [
                build_source_to_linear_lut(
                    read_tone_curve(&self.source_profile, TagSignature::RedTRCTag)?,
                    14,
                ),
                build_source_to_linear_lut(
                    read_tone_curve(&self.source_profile, TagSignature::GreenTRCTag)?,
                    14,
                ),
                build_source_to_linear_lut(
                    read_tone_curve(&self.source_profile, TagSignature::BlueTRCTag)?,
                    14,
                ),
            ],
            linear_to_source_u16: [
                build_linear_to_source_lut(
                    read_tone_curve(&self.source_profile, TagSignature::RedTRCTag)?,
                    14,
                ),
                build_linear_to_source_lut(
                    read_tone_curve(&self.source_profile, TagSignature::GreenTRCTag)?,
                    14,
                ),
                build_linear_to_source_lut(
                    read_tone_curve(&self.source_profile, TagSignature::BlueTRCTag)?,
                    14,
                ),
            ],
        })
    }

    pub fn to_linear_in_place(&self, image: &mut WorkingImage) -> Result<()> {
        if image.color_space == WorkingColorSpace::LinearRgb {
            return Ok(());
        }

        match &mut image.data {
            WorkingData::U8(rgba8) => {
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
            WorkingData::U16(rgba16) => {
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

    pub fn from_linear_in_place(&self, image: &mut WorkingImage) -> Result<()> {
        if image.color_space == WorkingColorSpace::Source {
            return Ok(());
        }

        match &mut image.data {
            WorkingData::U8(rgba8) => {
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
            WorkingData::U16(rgba16) => {
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

fn read_tone_curve<'a>(profile: &'a Profile, tag: TagSignature) -> Result<&'a lcms2::ToneCurveRef> {
    match profile.read_tag(tag) {
        Tag::ToneCurve(curve) => Ok(curve),
        _ => Err(Error::color(format!("ICC profile is missing {tag:?}"))),
    }
}

fn build_source_to_linear_lut(curve: &lcms2::ToneCurveRef, internal_bit_depth: u8) -> Vec<u16> {
    let max = (1u32 << internal_bit_depth) - 1;
    (0..=max)
        .map(|code| curve.eval(scale_code_to_u16(code, max)))
        .collect()
}

fn build_linear_to_source_lut(curve: &lcms2::ToneCurveRef, internal_bit_depth: u8) -> Vec<u16> {
    let inverse = curve.reversed_samples(65_536);
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

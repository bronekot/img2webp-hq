use bytemuck::cast_slice_mut;
use lcms2::{
    ColorSpaceSignature, Flags, Intent, PixelFormat, Profile, Tag, TagSignature, ToneCurve,
    Transform, XYZ2xyY,
};

use crate::decode::{WorkingData, WorkingImage};
use crate::error::{Error, Result};

pub struct ResizeColorPipeline {
    source_profile: Profile,
    linear_profile: Profile,
}

pub fn validate_source_profile(icc: Option<&[u8]>, require_matrix_shaper: bool) -> Result<()> {
    let Some(source_profile) = load_source_profile(icc)? else {
        return Ok(());
    };

    if require_matrix_shaper && !source_profile.is_matrix_shaper() {
        return Err(Error::unsupported(
            "only matrix-shaper RGB ICC profiles are supported for resize in v1",
        ));
    }

    Ok(())
}

impl ResizeColorPipeline {
    pub fn new(icc: Option<&[u8]>) -> Result<Self> {
        let source_profile = load_source_profile(icc)?.unwrap_or_else(Profile::new_srgb);
        if !source_profile.is_matrix_shaper() {
            return Err(Error::unsupported(
                "only matrix-shaper RGB ICC profiles are supported for resize in v1",
            ));
        }

        let linear_profile = make_linear_profile(&source_profile)?;

        Ok(Self {
            source_profile,
            linear_profile,
        })
    }

    pub fn to_linear_in_place(&self, image: &mut WorkingImage) -> Result<()> {
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
                Ok(())
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
                Ok(())
            }
        }
    }

    pub fn from_linear_in_place(&self, image: &mut WorkingImage) -> Result<()> {
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
                Ok(())
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
                Ok(())
            }
        }
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
                "ICC profile is missing MediaWhitePointTag for RGB resize pipeline",
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

use crate::error::{Error, Result};
use crate::metadata::MetadataPolicy;

/// Resampling filter used when image dimensions change.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeFilter {
    Lanczos3,
    CatmullRom,
    Mitchell,
}

/// WebP encoding mode.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Lossy,
    Lossless,
    NearLossless(u8),
}

/// Requested output dimensions and resize behavior.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct ResizeOptions {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub max_side: Option<u32>,
    pub no_upscale: bool,
    pub filter: Option<ResizeFilter>,
}

impl ResizeOptions {
    pub fn with_width(mut self, width: u32) -> Self {
        self.width = Some(width);
        self.max_width = None;
        self.max_height = None;
        self.max_side = None;
        self
    }

    pub fn with_height(mut self, height: u32) -> Self {
        self.height = Some(height);
        self.max_width = None;
        self.max_height = None;
        self.max_side = None;
        self
    }

    pub fn with_max_width(mut self, max_width: u32) -> Self {
        self.width = None;
        self.height = None;
        self.max_width = Some(max_width);
        self.max_side = None;
        self
    }

    pub fn with_max_height(mut self, max_height: u32) -> Self {
        self.width = None;
        self.height = None;
        self.max_height = Some(max_height);
        self.max_side = None;
        self
    }

    pub fn with_max_side(mut self, max_side: u32) -> Self {
        self.width = None;
        self.height = None;
        self.max_width = None;
        self.max_height = None;
        self.max_side = Some(max_side);
        self
    }

    pub fn with_no_upscale(mut self, no_upscale: bool) -> Self {
        self.no_upscale = no_upscale;
        self
    }

    pub fn with_filter(mut self, filter: ResizeFilter) -> Self {
        self.filter = Some(filter);
        self
    }

    fn validate(&self) -> Result<()> {
        if (self.width.is_some() || self.height.is_some())
            && (self.max_width.is_some() || self.max_height.is_some())
        {
            return Err(Error::invalid(
                "`--width/--height` cannot be combined with `--max-width/--max-height`",
            ));
        }
        if self.max_side.is_some()
            && (self.width.is_some()
                || self.height.is_some()
                || self.max_width.is_some()
                || self.max_height.is_some())
        {
            return Err(Error::invalid(
                "`--max-side` cannot be combined with other resize size flags",
            ));
        }
        Ok(())
    }
}

/// Complete configuration for one image conversion.
///
/// Start with [`ConversionOptions::default`] and modify public fields or use
/// the builder methods. Validation is performed by `convert` and
/// `convert_file` before any image processing starts.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ConversionOptions {
    pub mode: Mode,
    pub quality: f32,
    pub alpha_quality: u8,
    pub method: u8,
    pub sns_strength: Option<u8>,
    pub filter_strength: Option<u8>,
    pub exact: bool,
    pub sharpyuv: bool,
    pub fast: bool,
    pub fast_hq: bool,
    pub metadata: MetadataPolicy,
    pub resize: ResizeOptions,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            mode: Mode::Lossy,
            quality: 75.0,
            alpha_quality: 100,
            method: 4,
            sns_strength: None,
            filter_strength: None,
            exact: false,
            sharpyuv: false,
            fast: false,
            fast_hq: false,
            metadata: MetadataPolicy::Icc,
            resize: ResizeOptions::default(),
        }
    }
}

impl ConversionOptions {
    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_quality(mut self, quality: f32) -> Self {
        self.quality = quality;
        self
    }

    pub fn with_alpha_quality(mut self, alpha_quality: u8) -> Self {
        self.alpha_quality = alpha_quality;
        self
    }

    pub fn with_method(mut self, method: u8) -> Self {
        self.method = method;
        self
    }

    pub fn with_metadata(mut self, metadata: MetadataPolicy) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_resize(mut self, resize: ResizeOptions) -> Self {
        self.resize = resize;
        self
    }

    pub fn with_max_side(mut self, max_side: u32) -> Self {
        self.resize = self.resize.with_max_side(max_side);
        self
    }

    pub fn with_fast(mut self, fast: bool) -> Self {
        self.fast = fast;
        self
    }

    pub fn with_fast_hq(mut self, fast_hq: bool) -> Self {
        self.fast_hq = fast_hq;
        self
    }

    pub fn with_sharpyuv(mut self, sharpyuv: bool) -> Self {
        self.sharpyuv = sharpyuv;
        self
    }

    pub fn validate(&self) -> Result<()> {
        if self.fast && self.fast_hq {
            return Err(Error::invalid(
                "`--fast` and `--fasthq` cannot be used together",
            ));
        }
        if self.fast_hq && self.sharpyuv {
            return Err(Error::invalid(
                "`--fasthq` cannot be combined with `--sharpyuv`",
            ));
        }
        if self.fast_hq && !matches!(self.mode, Mode::Lossy) {
            return Err(Error::invalid(
                "`--fasthq` is available only for lossy encoding",
            ));
        }
        if self.method > 6 {
            return Err(Error::invalid("`-m` must be in the range 0..=6"));
        }
        if self.alpha_quality > 100 {
            return Err(Error::invalid("`-alpha_q` must be in the range 0..=100"));
        }
        if let Mode::NearLossless(value) = self.mode
            && value > 100
        {
            return Err(Error::invalid(
                "`-near_lossless` must be in the range 0..=100",
            ));
        }
        if !(0.0..=100.0).contains(&self.quality) {
            return Err(Error::invalid("`-q` must be in the range 0..=100"));
        }
        if let Some(value) = self.sns_strength
            && value > 100
        {
            return Err(Error::invalid("`-sns` must be in the range 0..=100"));
        }
        if let Some(value) = self.filter_strength
            && value > 100
        {
            return Err(Error::invalid("`-f` must be in the range 0..=100"));
        }
        self.resize.validate()
    }

    pub(crate) fn uses_simpleyuv(&self) -> bool {
        !self.sharpyuv
    }

    pub(crate) fn uses_fast_decode(&self) -> bool {
        self.fast
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_cli_defaults() {
        let options = ConversionOptions::default();
        assert_eq!(options.mode, Mode::Lossy);
        assert_eq!(options.quality, 75.0);
        assert_eq!(options.alpha_quality, 100);
        assert_eq!(options.method, 4);
        assert_eq!(options.metadata, MetadataPolicy::Icc);
        options.validate().unwrap();
    }

    #[test]
    fn max_side_builder_replaces_other_dimensions() {
        let options = ResizeOptions::default()
            .with_width(800)
            .with_height(600)
            .with_max_side(1200);
        assert_eq!(options.max_side, Some(1200));
        assert_eq!(options.width, None);
        assert_eq!(options.height, None);
        options.validate().unwrap();
    }
}

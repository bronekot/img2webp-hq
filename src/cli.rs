use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use crate::error::{Error, Result};
use crate::metadata::MetadataPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum MetadataArg {
    None,
    All,
    Exif,
    Icc,
    Xmp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ResizeFilterArg {
    Lanczos3,
    Catmullrom,
    Mitchell,
}

#[derive(Debug, Clone, Parser)]
#[command(
    name = "img2webp-hq",
    version,
    about = "High-quality image to WebP converter"
)]
struct Cli {
    input: PathBuf,

    #[arg(short = 'o', long = "output")]
    output: PathBuf,

    #[arg(short = 'q', default_value_t = 75.0)]
    quality: f32,

    #[arg(long = "alpha_q", default_value_t = 100)]
    alpha_quality: u8,

    #[arg(short = 'm', default_value_t = 4)]
    method: u8,

    #[arg(long = "sns")]
    sns_strength: Option<u8>,

    #[arg(short = 'f')]
    filter_strength: Option<u8>,

    #[arg(
        long = "sharpyuv",
        alias = "sharp_yuv",
        action = clap::ArgAction::SetTrue
    )]
    sharpyuv: bool,

    #[arg(long = "lossless", action = clap::ArgAction::SetTrue)]
    lossless: bool,

    #[arg(long = "near_lossless")]
    near_lossless: Option<u8>,

    #[arg(long = "exact", action = clap::ArgAction::SetTrue)]
    exact: bool,

    #[arg(long = "fast", action = clap::ArgAction::SetTrue)]
    fast: bool,

    #[arg(long = "fasthq", alias = "fast-hq", action = clap::ArgAction::SetTrue)]
    fast_hq: bool,

    #[arg(long = "metadata", value_enum, default_value_t = MetadataArg::Icc)]
    metadata: MetadataArg,

    #[arg(long = "width")]
    width: Option<u32>,

    #[arg(long = "height")]
    height: Option<u32>,

    #[arg(long = "max-width")]
    max_width: Option<u32>,

    #[arg(long = "max-height")]
    max_height: Option<u32>,

    #[arg(long = "max-side", alias = "longest-side")]
    max_side: Option<u32>,

    #[arg(long = "no-upscale", action = clap::ArgAction::SetTrue)]
    no_upscale: bool,

    #[arg(long = "filter", value_enum)]
    resize_filter: Option<ResizeFilterArg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeFilter {
    Lanczos3,
    CatmullRom,
    Mitchell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Lossy,
    Lossless,
    NearLossless(u8),
}

#[derive(Debug, Clone)]
pub struct ResizeOptions {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub max_side: Option<u32>,
    pub no_upscale: bool,
    pub filter: Option<ResizeFilter>,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub input: PathBuf,
    pub output: PathBuf,
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

impl Job {
    pub fn uses_simpleyuv(&self) -> bool {
        !self.sharpyuv
    }

    pub fn uses_fast_decode(&self) -> bool {
        self.fast
    }
}

pub fn parse_from_env() -> Result<Job> {
    let args = normalize_cwebp_style_args(std::env::args_os());
    let cli = Cli::parse_from(args);
    validate(cli)
}

fn validate(cli: Cli) -> Result<Job> {
    if cli.lossless && cli.near_lossless.is_some() {
        return Err(Error::invalid(
            "`-lossless` and `-near_lossless` cannot be used together",
        ));
    }
    if cli.fast && cli.fast_hq {
        return Err(Error::invalid(
            "`--fast` and `--fasthq` cannot be used together",
        ));
    }
    if (cli.width.is_some() || cli.height.is_some())
        && (cli.max_width.is_some() || cli.max_height.is_some())
    {
        return Err(Error::invalid(
            "`--width/--height` cannot be combined with `--max-width/--max-height`",
        ));
    }
    if cli.max_side.is_some()
        && (cli.width.is_some()
            || cli.height.is_some()
            || cli.max_width.is_some()
            || cli.max_height.is_some())
    {
        return Err(Error::invalid(
            "`--max-side` cannot be combined with other resize size flags",
        ));
    }
    if cli.method > 6 {
        return Err(Error::invalid("`-m` must be in the range 0..=6"));
    }
    if cli.alpha_quality > 100 {
        return Err(Error::invalid("`-alpha_q` must be in the range 0..=100"));
    }
    if let Some(value) = cli.near_lossless
        && value > 100
    {
        return Err(Error::invalid(
            "`-near_lossless` must be in the range 0..=100",
        ));
    }
    if !(0.0..=100.0).contains(&cli.quality) {
        return Err(Error::invalid("`-q` must be in the range 0..=100"));
    }
    if let Some(value) = cli.sns_strength
        && value > 100
    {
        return Err(Error::invalid("`-sns` must be in the range 0..=100"));
    }
    if let Some(value) = cli.filter_strength
        && value > 100
    {
        return Err(Error::invalid("`-f` must be in the range 0..=100"));
    }

    let mode = if cli.lossless {
        Mode::Lossless
    } else if let Some(value) = cli.near_lossless {
        Mode::NearLossless(value)
    } else {
        Mode::Lossy
    };

    let metadata = match cli.metadata {
        MetadataArg::None => MetadataPolicy::None,
        MetadataArg::All => MetadataPolicy::All,
        MetadataArg::Exif => MetadataPolicy::Exif,
        MetadataArg::Icc => MetadataPolicy::Icc,
        MetadataArg::Xmp => MetadataPolicy::Xmp,
    };

    let filter = match cli.resize_filter {
        Some(ResizeFilterArg::Lanczos3) => Some(ResizeFilter::Lanczos3),
        Some(ResizeFilterArg::Catmullrom) => Some(ResizeFilter::CatmullRom),
        Some(ResizeFilterArg::Mitchell) => Some(ResizeFilter::Mitchell),
        None => None,
    };

    return_job(cli, mode, metadata, filter)
}

fn return_job(
    cli: Cli,
    mode: Mode,
    metadata: MetadataPolicy,
    filter: Option<ResizeFilter>,
) -> Result<Job> {
    Ok(Job {
        input: cli.input,
        output: cli.output,
        mode,
        quality: cli.quality,
        alpha_quality: cli.alpha_quality,
        method: cli.method,
        sns_strength: cli.sns_strength,
        filter_strength: cli.filter_strength,
        exact: cli.exact,
        sharpyuv: cli.sharpyuv,
        fast: cli.fast,
        fast_hq: cli.fast_hq,
        metadata,
        resize: ResizeOptions {
            width: cli.width,
            height: cli.height,
            max_width: cli.max_width,
            max_height: cli.max_height,
            max_side: cli.max_side,
            no_upscale: cli.no_upscale,
            filter,
        },
    })
}

fn normalize_cwebp_style_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    const REWRITE: &[&str] = &[
        "alpha_q",
        "sharpyuv",
        "sharp_yuv",
        "lossless",
        "near_lossless",
        "exact",
        "fast",
        "fasthq",
        "metadata",
    ];

    args.into_iter()
        .enumerate()
        .map(|(index, arg)| {
            if index == 0 {
                return arg;
            }

            let Some(value) = arg.to_str() else {
                return arg;
            };

            if let Some(name) = REWRITE.iter().find(|name| value == format!("-{name}")) {
                OsString::from(format!("--{name}"))
            } else {
                arg
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_cwebp_style_long_flags() {
        let args = normalize_cwebp_style_args([
            OsString::from("img2webp-hq"),
            OsString::from("-metadata"),
            OsString::from("icc"),
            OsString::from("-sharpyuv"),
            OsString::from("-sharp_yuv"),
            OsString::from("-lossless"),
            OsString::from("-fast"),
            OsString::from("-fasthq"),
        ]);
        let values: Vec<_> = args
            .into_iter()
            .map(|item| item.into_string().unwrap())
            .collect();
        assert_eq!(values[1], "--metadata");
        assert_eq!(values[3], "--sharpyuv");
        assert_eq!(values[4], "--sharp_yuv");
        assert_eq!(values[5], "--lossless");
        assert_eq!(values[6], "--fast");
        assert_eq!(values[7], "--fasthq");
    }

    #[test]
    fn rejects_max_side_with_other_size_flags() {
        let err = validate(Cli {
            input: PathBuf::from("in.png"),
            output: PathBuf::from("out.webp"),
            quality: 75.0,
            alpha_quality: 100,
            method: 4,
            sns_strength: None,
            filter_strength: None,
            sharpyuv: false,
            lossless: false,
            near_lossless: None,
            exact: false,
            fast: false,
            fast_hq: false,
            metadata: MetadataArg::Icc,
            width: Some(1200),
            height: None,
            max_width: None,
            max_height: None,
            max_side: Some(1200),
            no_upscale: false,
            resize_filter: None,
        })
        .unwrap_err();

        assert_eq!(
            err.to_string(),
            "`--max-side` cannot be combined with other resize size flags"
        );
    }

    #[test]
    fn rejects_fast_and_fasthq_together() {
        let err = validate(Cli {
            input: PathBuf::from("in.png"),
            output: PathBuf::from("out.webp"),
            quality: 75.0,
            alpha_quality: 100,
            method: 4,
            sns_strength: None,
            filter_strength: None,
            sharpyuv: false,
            lossless: false,
            near_lossless: None,
            exact: false,
            fast: true,
            fast_hq: true,
            metadata: MetadataArg::Icc,
            width: None,
            height: None,
            max_width: None,
            max_height: None,
            max_side: None,
            no_upscale: false,
            resize_filter: None,
        })
        .unwrap_err();

        assert_eq!(
            err.to_string(),
            "`--fast` and `--fasthq` cannot be used together"
        );
    }

    #[test]
    fn sharpyuv_flag_switches_lossy_pipeline() {
        let job = validate(Cli {
            input: PathBuf::from("in.png"),
            output: PathBuf::from("out.webp"),
            quality: 75.0,
            alpha_quality: 100,
            method: 4,
            sns_strength: None,
            filter_strength: None,
            sharpyuv: true,
            lossless: false,
            near_lossless: None,
            exact: false,
            fast: false,
            fast_hq: false,
            metadata: MetadataArg::Icc,
            width: None,
            height: None,
            max_width: None,
            max_height: None,
            max_side: None,
            no_upscale: false,
            resize_filter: None,
        })
        .unwrap();

        assert!(job.sharpyuv);
        assert!(!job.uses_simpleyuv());
        assert!(!job.uses_fast_decode());
    }
}

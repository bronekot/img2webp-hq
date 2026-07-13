use std::fs::{self, File};
use std::path::Path;
use std::time::Instant;
use std::{hint::black_box, path::PathBuf};

use image::codecs::png::PngEncoder;
use image::metadata::Orientation;
use image::{ExtendedColorType, ImageEncoder, ImageFormat};
use lcms2::{CIExyY, Profile, ToneCurve};
use tempfile::tempdir;
use webpx::{get_exif, get_icc_profile};

use crate::cli::{Job, Mode, ResizeOptions};
use crate::cms::ColorPipeline;
use crate::decode::{self, WorkingData, WorkingImage};
use crate::error::Error;
use crate::metadata::MetadataPolicy;
use crate::pipeline;
use crate::{sharpyuv, simpleyuv};

fn base_job(input: &Path, output: &Path) -> Job {
    Job {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        mode: Mode::Lossless,
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
        resize: ResizeOptions {
            width: None,
            height: None,
            max_width: None,
            max_height: None,
            max_side: None,
            no_upscale: false,
            filter: None,
        },
    }
}

fn expand_u8_to_u16(data: &[u8]) -> Vec<u16> {
    data.iter().map(|&value| (value as u16) * 257).collect()
}

fn downcast_u16_to_u8(data: &[u16]) -> Vec<u8> {
    data.iter()
        .map(|&value| ((value as u32 + 128) / 257) as u8)
        .collect()
}

fn max_abs_diff_u8(left: &[u8], right: &[u8]) -> i32 {
    left.iter()
        .zip(right)
        .map(|(&l, &r)| (l as i32 - r as i32).abs())
        .max()
        .unwrap_or(0)
}

fn max_abs_diff_u16(left: &[u16], right: &[u16]) -> i32 {
    left.iter()
        .zip(right)
        .map(|(&left, &right)| (left as i32 - right as i32).abs())
        .max()
        .unwrap_or(0)
}

fn rgb_psnr(reference: &[u8], candidate: &[u8]) -> f64 {
    let (squared_error, samples) = reference
        .chunks_exact(4)
        .zip(candidate.chunks_exact(4))
        .fold((0.0, 0usize), |(error, samples), (reference, candidate)| {
            let pixel_error = (0..3)
                .map(|channel| {
                    let delta = reference[channel] as f64 - candidate[channel] as f64;
                    delta * delta
                })
                .sum::<f64>();
            (error + pixel_error, samples + 3)
        });
    if squared_error == 0.0 {
        return f64::INFINITY;
    }
    let mse = squared_error / samples as f64;
    10.0 * (255.0 * 255.0 / mse).log10()
}

fn luma_dssim(reference: &[u8], candidate: &[u8], width: u32, height: u32) -> f64 {
    const WINDOW: usize = 8;
    const C1: f64 = 6.5025;
    const C2: f64 = 58.5225;

    let width = width as usize;
    let height = height as usize;
    let mut ssim_sum = 0.0;
    let mut windows = 0usize;
    for y0 in (0..height).step_by(WINDOW) {
        for x0 in (0..width).step_by(WINDOW) {
            let x1 = (x0 + WINDOW).min(width);
            let y1 = (y0 + WINDOW).min(height);
            let count = (x1 - x0) * (y1 - y0);
            let mut reference_sum = 0.0;
            let mut candidate_sum = 0.0;
            let mut reference_sq_sum = 0.0;
            let mut candidate_sq_sum = 0.0;
            let mut product_sum = 0.0;

            for y in y0..y1 {
                for x in x0..x1 {
                    let offset = (y * width + x) * 4;
                    let reference_luma = rgb_luma(&reference[offset..offset + 3]);
                    let candidate_luma = rgb_luma(&candidate[offset..offset + 3]);
                    reference_sum += reference_luma;
                    candidate_sum += candidate_luma;
                    reference_sq_sum += reference_luma * reference_luma;
                    candidate_sq_sum += candidate_luma * candidate_luma;
                    product_sum += reference_luma * candidate_luma;
                }
            }

            let count = count as f64;
            let reference_mean = reference_sum / count;
            let candidate_mean = candidate_sum / count;
            let reference_variance = reference_sq_sum / count - reference_mean * reference_mean;
            let candidate_variance = candidate_sq_sum / count - candidate_mean * candidate_mean;
            let covariance = product_sum / count - reference_mean * candidate_mean;
            let ssim = ((2.0 * reference_mean * candidate_mean + C1) * (2.0 * covariance + C2))
                / ((reference_mean * reference_mean + candidate_mean * candidate_mean + C1)
                    * (reference_variance + candidate_variance + C2));
            ssim_sum += ssim;
            windows += 1;
        }
    }

    (1.0 - ssim_sum / windows as f64) / 2.0
}

fn rgb_luma(pixel: &[u8]) -> f64 {
    0.299 * pixel[0] as f64 + 0.587 * pixel[1] as f64 + 0.114 * pixel[2] as f64
}

fn write_png_rgba(
    path: &Path,
    width: u32,
    height: u32,
    pixels: &[u8],
    icc: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
) {
    let file = File::create(path).unwrap();
    let mut encoder = PngEncoder::new(file);
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc).unwrap();
    }
    if let Some(exif) = exif {
        encoder.set_exif_metadata(exif).unwrap();
    }
    encoder
        .write_image(pixels, width, height, ExtendedColorType::Rgba8)
        .unwrap();
}

fn write_png_gray(path: &Path, width: u32, height: u32, pixels: &[u8], icc: Option<Vec<u8>>) {
    let file = File::create(path).unwrap();
    let mut encoder = PngEncoder::new(file);
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc).unwrap();
    }
    encoder
        .write_image(pixels, width, height, ExtendedColorType::L8)
        .unwrap();
}

type DecodedWebp = (u32, u32, Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>);

fn read_webp(path: &Path) -> DecodedWebp {
    let bytes = fs::read(path).unwrap();
    let rgba = image::load_from_memory_with_format(&bytes, ImageFormat::WebP)
        .unwrap()
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    let pixels = rgba.into_raw();
    let icc = get_icc_profile(&bytes).unwrap();
    let exif = get_exif(&bytes).unwrap();
    (width, height, pixels, icc, exif)
}

fn exif_with_orientation(orientation: Orientation) -> Vec<u8> {
    let value = orientation.to_exif() as u16;
    vec![
        0x49,
        0x49,
        42,
        0,
        8,
        0,
        0,
        0,
        1,
        0,
        0x12,
        0x01,
        3,
        0,
        1,
        0,
        0,
        0,
        value as u8,
        (value >> 8) as u8,
        0,
        0,
        0,
        0,
        0,
        0,
    ]
}

#[test]
fn lossless_roundtrips_pixels_and_preserves_icc() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("input.png");
    let output = dir.path().join("output.webp");
    let icc = Profile::new_srgb().icc().unwrap();
    let pixels = vec![
        255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 255, 255, 255, 255,
    ];

    write_png_rgba(&input, 2, 2, &pixels, Some(icc), None);
    let decoded_input = decode::decode(&input, false).unwrap();
    assert!(decoded_input.metadata.icc.is_some());

    let job = base_job(&input, &output);
    pipeline::run(job).unwrap();

    let (width, height, output_pixels, output_icc, output_exif) = read_webp(&output);
    assert_eq!((width, height), (2, 2));
    assert_eq!(output_pixels, pixels);
    assert!(output_icc.is_some());
    assert!(output_exif.is_none());
}

#[test]
fn all_modes_produce_decodable_webp_output() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("input.png");
    let pixels = vec![
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 64,
    ];

    write_png_rgba(&input, 2, 2, &pixels, None, None);

    for (name, mode) in [
        ("lossy", Mode::Lossy),
        ("lossless", Mode::Lossless),
        ("near-lossless", Mode::NearLossless(60)),
    ] {
        let output = dir.path().join(format!("{name}.webp"));
        let mut job = base_job(&input, &output);
        job.mode = mode;
        job.metadata = MetadataPolicy::None;
        job.resize.width = Some(4);
        job.resize.height = Some(4);
        job.quality = 80.0;

        pipeline::run(job).unwrap();

        let (width, height, _pixels, icc, exif) = read_webp(&output);
        assert_eq!((width, height), (4, 4), "{name}");
        assert!(icc.is_none(), "{name}");
        assert!(exif.is_none(), "{name}");
    }

    let sharpyuv_output = dir.path().join("lossy-sharpyuv.webp");
    let mut sharpyuv_job = base_job(&input, &sharpyuv_output);
    sharpyuv_job.mode = Mode::Lossy;
    sharpyuv_job.sharpyuv = true;
    sharpyuv_job.metadata = MetadataPolicy::None;
    sharpyuv_job.resize.width = Some(4);
    sharpyuv_job.resize.height = Some(4);
    sharpyuv_job.quality = 80.0;

    pipeline::run(sharpyuv_job).unwrap();

    let (width, height, _pixels, icc, exif) = read_webp(&sharpyuv_output);
    assert_eq!((width, height), (4, 4), "lossy-sharpyuv");
    assert!(icc.is_none(), "lossy-sharpyuv");
    assert!(exif.is_none(), "lossy-sharpyuv");
}

#[test]
fn applies_orientation_and_clears_exif_metadata() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("rotated.png");
    let output = dir.path().join("rotated.webp");
    let pixels = vec![255, 0, 0, 255, 0, 0, 255, 255];

    write_png_rgba(
        &input,
        2,
        1,
        &pixels,
        None,
        Some(exif_with_orientation(Orientation::Rotate90)),
    );
    let decoded_input = decode::decode(&input, false).unwrap();
    assert_eq!(decoded_input.metadata.orientation, Orientation::Rotate90);
    assert!(decoded_input.metadata.exif.is_some());

    let mut job = base_job(&input, &output);
    job.metadata = MetadataPolicy::All;

    pipeline::run(job).unwrap();

    let (width, height, output_pixels, _icc, exif) = read_webp(&output);
    assert_eq!((width, height), (1, 2));
    assert_eq!(output_pixels, vec![255, 0, 0, 255, 0, 0, 255, 255]);

    let exif = exif.expect("expected EXIF metadata to be preserved");
    assert_eq!(
        Orientation::from_exif_chunk(&exif),
        Some(Orientation::NoTransforms)
    );
}

#[test]
fn rejects_non_rgb_icc_profile() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("gray.png");
    let output = dir.path().join("gray.webp");
    let gray_profile = Profile::new_gray(
        &CIExyY {
            x: 0.3127,
            y: 0.3290,
            Y: 1.0,
        },
        &ToneCurve::new(2.2),
    )
    .unwrap()
    .icc()
    .unwrap();

    write_png_gray(&input, 1, 1, &[128], Some(gray_profile));

    let job = base_job(&input, &output);
    let err = pipeline::run(job).unwrap_err();
    match err {
        Error::Unsupported(message) => {
            assert!(message.contains("only RGB ICC profiles are supported"));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn fast_mode_decodes_and_processes_in_u8() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("fast.png");
    let output = dir.path().join("fast.webp");
    let pixels = vec![
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];

    write_png_rgba(&input, 2, 2, &pixels, None, None);

    let decoded_input = decode::decode(&input, true).unwrap();
    assert!(decoded_input.image.is_u8());

    let mut job = base_job(&input, &output);
    job.fast = true;
    job.mode = Mode::Lossy;
    job.metadata = MetadataPolicy::None;
    job.resize.width = Some(4);
    job.resize.height = Some(4);

    pipeline::run(job).unwrap();

    let (width, height, _pixels, _icc, _exif) = read_webp(&output);
    assert_eq!((width, height), (4, 4));
}

#[test]
fn fasthq_mode_keeps_high_bit_depth_pipeline() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("fasthq.png");
    let output = dir.path().join("fasthq.webp");
    let pixels = vec![
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];

    write_png_rgba(&input, 2, 2, &pixels, None, None);

    let decoded_input = decode::decode(&input, false).unwrap();
    assert!(!decoded_input.image.is_u8());

    let mut job = base_job(&input, &output);
    job.fast_hq = true;
    job.mode = Mode::Lossy;
    job.metadata = MetadataPolicy::None;
    job.resize.width = Some(4);
    job.resize.height = Some(4);

    pipeline::run(job).unwrap();

    let (width, height, _pixels, _icc, _exif) = read_webp(&output);
    assert_eq!((width, height), (4, 4));
}

#[test]
fn fast_mode_stays_close_to_default_simpleyuv_path() {
    let dir = tempdir().unwrap();
    let input = Path::new("test/1.jpeg");
    let normal_output = dir.path().join("normal.webp");
    let fast_output = dir.path().join("fast.webp");

    let mut normal_job = base_job(input, &normal_output);
    normal_job.fast = false;
    normal_job.mode = Mode::Lossy;
    normal_job.quality = 100.0;
    normal_job.metadata = MetadataPolicy::None;

    let mut fast_job = base_job(input, &fast_output);
    fast_job.fast = true;
    fast_job.mode = Mode::Lossy;
    fast_job.quality = 100.0;
    fast_job.metadata = MetadataPolicy::None;

    pipeline::run(normal_job).unwrap();
    pipeline::run(fast_job).unwrap();

    let (_w, _h, normal_pixels, _, _) = read_webp(&normal_output);
    let (_w, _h, fast_pixels, _, _) = read_webp(&fast_output);

    let max_diff = normal_pixels
        .iter()
        .zip(fast_pixels.iter())
        .map(|(n, f)| (*n as i32 - *f as i32).abs())
        .max()
        .unwrap();

    // Fast mode now uses the same SimpleYUV path as default lossy mode, but on
    // top of an 8-bit decode path instead of the default high-bit-depth one.
    assert!(
        max_diff <= 10,
        "Fast mode should stay close to default SimpleYUV mode (max diff: {max_diff})"
    );
}

#[test]
fn fasthq_no_longer_produces_dark_output_after_resize() {
    // The bug: before fix, resize left image in LinearRgb and fasthq sharpish path
    // treated linear data as sRGB, producing dark output.
    // After fix: image properly converts back to sRGB after resize.
    let dir = tempdir().unwrap();
    let input = Path::new("test/1.jpeg");
    let fasthq_output = dir.path().join("fasthq.webp");

    let mut fasthq_job = base_job(input, &fasthq_output);
    fasthq_job.fast_hq = true;
    fasthq_job.mode = Mode::Lossy;
    fasthq_job.quality = 70.0;
    fasthq_job.method = 6;
    fasthq_job.metadata = MetadataPolicy::None;
    fasthq_job.resize.max_side = Some(600);

    pipeline::run(fasthq_job).unwrap();

    let (_, _, fasthq_pixels, _, _) = read_webp(&fasthq_output);

    let avg_luma = fasthq_pixels
        .chunks_exact(4)
        .map(|pixel| {
            let r = pixel[0] as f64;
            let g = pixel[1] as f64;
            let b = pixel[2] as f64;
            0.299 * r + 0.587 * g + 0.114 * b
        })
        .sum::<f64>()
        / (fasthq_pixels.len() / 4) as f64;

    assert!(
        avg_luma > 30.0,
        "Fast HQ output should not be dark after resize (avg_luma={avg_luma:.1})",
    );
}

#[test]
#[ignore = "quality regression gate; run explicitly in release mode"]
fn fasthq_resize_quality_and_size_gate() {
    let dir = tempdir().unwrap();
    let input = Path::new("test/2.jpeg");
    let reference_output = dir.path().join("reference.webp");

    let mut reference_job = base_job(input, &reference_output);
    reference_job.mode = Mode::Lossless;
    reference_job.metadata = MetadataPolicy::None;
    reference_job.resize.max_side = Some(1200);
    pipeline::run(reference_job).unwrap();
    let (width, height, reference_pixels, _, _) = read_webp(&reference_output);

    for method in [0, 4] {
        let default_output = dir.path().join(format!("default-m{method}.webp"));
        let fast_hq_output = dir.path().join(format!("fasthq-m{method}.webp"));

        let mut default_job = base_job(input, &default_output);
        default_job.mode = Mode::Lossy;
        default_job.method = method;
        default_job.metadata = MetadataPolicy::None;
        default_job.resize.max_side = Some(1200);
        pipeline::run(default_job).unwrap();

        let mut fast_hq_job = base_job(input, &fast_hq_output);
        fast_hq_job.mode = Mode::Lossy;
        fast_hq_job.method = method;
        fast_hq_job.fast_hq = true;
        fast_hq_job.metadata = MetadataPolicy::None;
        fast_hq_job.resize.max_side = Some(1200);
        pipeline::run(fast_hq_job).unwrap();

        let (_, _, default_pixels, _, _) = read_webp(&default_output);
        let (_, _, fast_hq_pixels, _, _) = read_webp(&fast_hq_output);
        let default_psnr = rgb_psnr(&reference_pixels, &default_pixels);
        let fast_hq_psnr = rgb_psnr(&reference_pixels, &fast_hq_pixels);
        let default_dssim = luma_dssim(&reference_pixels, &default_pixels, width, height);
        let fast_hq_dssim = luma_dssim(&reference_pixels, &fast_hq_pixels, width, height);
        let default_size = fs::metadata(&default_output).unwrap().len();
        let fast_hq_size = fs::metadata(&fast_hq_output).unwrap().len();

        eprintln!(
            "fasthq m{method}: default psnr={default_psnr:.4} dssim={default_dssim:.8} size={default_size}; fast psnr={fast_hq_psnr:.4} dssim={fast_hq_dssim:.8} size={fast_hq_size}"
        );
        assert!(
            fast_hq_psnr >= default_psnr,
            "PSNR regressed for -m {method}"
        );
        assert!(
            fast_hq_dssim <= default_dssim,
            "DSSIM regressed for -m {method}"
        );
        assert!(
            fast_hq_size <= default_size,
            "output size increased for -m {method}"
        );
    }
}

#[test]
fn fasthq_yuv_planes_match_default_pipeline() {
    let decoded = decode::decode(Path::new("test/2.jpeg"), false).unwrap();
    let cms = ColorPipeline::new(decoded.metadata.icc.as_deref()).unwrap();
    let target = crate::resize::compute_target_size(
        decoded.image.width,
        decoded.image.height,
        &ResizeOptions {
            width: None,
            height: None,
            max_width: None,
            max_height: None,
            max_side: Some(1200),
            no_upscale: false,
            filter: None,
        },
    )
    .unwrap();
    let filter =
        crate::resize::resolve_filter(decoded.image.width, decoded.image.height, target, None);

    let mut default_image = decoded.image.clone();
    cms.to_linear_in_place(&mut default_image).unwrap();
    let mut fast_image = decoded.image;
    cms.to_linear_parallel_in_place(&mut fast_image).unwrap();
    let default_linear = match &default_image.data {
        WorkingData::Rgb16(data) => data,
        _ => unreachable!(),
    };
    let fast_linear = match &fast_image.data {
        WorkingData::Rgb16(data) => data,
        _ => unreachable!(),
    };
    assert_eq!(max_abs_diff_u16(default_linear, fast_linear), 0);

    default_image = crate::resize::resize(default_image, target, filter).unwrap();
    fast_image = crate::resize::resize(fast_image, target, filter).unwrap();
    let default_linear = match &default_image.data {
        WorkingData::Rgb16(data) => data,
        _ => unreachable!(),
    };
    let fast_linear = match &fast_image.data {
        WorkingData::Rgb16(data) => data,
        _ => unreachable!(),
    };
    assert_eq!(max_abs_diff_u16(default_linear, fast_linear), 0);

    cms.convert_from_linear_in_place(&mut default_image)
        .unwrap();
    cms.convert_from_linear_parallel_in_place(&mut fast_image)
        .unwrap();
    let default_source = match &default_image.data {
        WorkingData::Rgb16(data) => data,
        _ => unreachable!(),
    };
    let fast_source = match &fast_image.data {
        WorkingData::Rgb16(data) => data,
        _ => unreachable!(),
    };
    assert_eq!(max_abs_diff_u16(default_source, fast_source), 0);

    let default = simpleyuv::working_image_to_yuv420(&default_image, &cms).unwrap();
    let fast = simpleyuv::working_image_to_yuv420(&fast_image, &cms).unwrap();

    assert_eq!(max_abs_diff_u8(&fast.y, &default.y), 0, "Y planes differ");
    assert_eq!(max_abs_diff_u8(&fast.u, &default.u), 0, "U planes differ");
    assert_eq!(max_abs_diff_u8(&fast.v, &default.v), 0, "V planes differ");
    assert_eq!(fast.a, default.a);
}

#[test]
fn jpeg_u8_and_u16_simpleyuv_paths_stay_aligned() {
    for input in [Path::new("test/1.jpeg"), Path::new("test/2.jpeg")] {
        let decoded_u8 = decode::decode(input, true).unwrap();
        let decoded_u16 = decode::decode(input, false).unwrap();
        let cms = ColorPipeline::new(decoded_u8.metadata.icc.as_deref()).unwrap();

        let data_u8 = match &decoded_u8.image.data {
            WorkingData::Rgb8(data) => data,
            _ => panic!("expected RGB8 decode"),
        };
        let data_u16 = match &decoded_u16.image.data {
            WorkingData::Rgb16(data) => data,
            _ => panic!("expected RGB16 decode"),
        };

        let expanded_u8 = expand_u8_to_u16(data_u8);
        let downcast_u16 = downcast_u16_to_u8(data_u16);

        let max_fast_vs_downcast = max_abs_diff_u8(data_u8, &downcast_u16);

        let expanded_image = WorkingImage {
            width: decoded_u8.image.width,
            height: decoded_u8.image.height,
            data: WorkingData::Rgb16(expanded_u8),
            color_space: decoded_u8.image.color_space,
        };
        let downcast_image = WorkingImage {
            width: decoded_u16.image.width,
            height: decoded_u16.image.height,
            data: WorkingData::Rgb8(downcast_u16),
            color_space: decoded_u16.image.color_space,
        };

        let planes_u8 = simpleyuv::working_image_to_yuv420(&decoded_u8.image, &cms).unwrap();
        let planes_expanded = simpleyuv::working_image_to_yuv420(&expanded_image, &cms).unwrap();
        let planes_downcast = simpleyuv::working_image_to_yuv420(&downcast_image, &cms).unwrap();
        let planes_u16 = simpleyuv::working_image_to_yuv420(&decoded_u16.image, &cms).unwrap();

        assert!(
            max_fast_vs_downcast <= 1,
            "downcasted u16 decode should match u8 decode for {} (max diff {})",
            input.display(),
            max_fast_vs_downcast,
        );
        assert!(
            max_abs_diff_u8(&planes_u8.y, &planes_expanded.y) <= 1
                && max_abs_diff_u8(&planes_u8.u, &planes_expanded.u) <= 1
                && max_abs_diff_u8(&planes_u8.v, &planes_expanded.v) <= 1,
            "simpleyuv should keep u8 and equivalent u16 samples aligned for {}",
            input.display(),
        );
        assert_eq!(max_abs_diff_u8(&planes_u8.y, &planes_downcast.y), 0);
        assert_eq!(max_abs_diff_u8(&planes_u8.u, &planes_downcast.u), 0);
        assert_eq!(max_abs_diff_u8(&planes_u8.v, &planes_downcast.v), 0);
        assert!(
            max_abs_diff_u8(&planes_u8.y, &planes_u16.y) <= 1
                && max_abs_diff_u8(&planes_u8.u, &planes_u16.u) <= 1
                && max_abs_diff_u8(&planes_u8.v, &planes_u16.v) <= 1,
            "simpleyuv u16 path drifted too far from u8 path for {}",
            input.display(),
        );
    }
}

#[test]
#[ignore]
fn bench_simpleyuv_vs_sharpyuv_on_same_u8_input() {
    let cases: [(PathBuf, usize); 2] = [
        (PathBuf::from("test/1.jpeg"), 200),
        (PathBuf::from("test/2.jpeg"), 20),
    ];

    for (path, iterations) in cases {
        let decoded = decode::decode(&path, true).unwrap();
        let cms = ColorPipeline::new(decoded.metadata.icc.as_deref()).unwrap();
        for _ in 0..5 {
            black_box(simpleyuv::working_image_to_yuv420(&decoded.image, &cms).unwrap());
            black_box(sharpyuv::working_image_to_yuv420(&decoded.image, false).unwrap());
        }

        let start = Instant::now();
        for _ in 0..iterations {
            black_box(simpleyuv::working_image_to_yuv420(&decoded.image, &cms).unwrap());
        }
        let simpleyuv_elapsed = start.elapsed();

        let start = Instant::now();
        for _ in 0..iterations {
            black_box(sharpyuv::working_image_to_yuv420(&decoded.image, false).unwrap());
        }
        let sharpyuv_elapsed = start.elapsed();

        let simpleyuv_ms = simpleyuv_elapsed.as_secs_f64() * 1000.0 / iterations as f64;
        let sharpyuv_ms = sharpyuv_elapsed.as_secs_f64() * 1000.0 / iterations as f64;

        eprintln!(
            "{} iterations={} simpleyuv={:.3}ms sharpyuv={:.3}ms ratio={:.2}x",
            path.display(),
            iterations,
            simpleyuv_ms,
            sharpyuv_ms,
            simpleyuv_ms / sharpyuv_ms,
        );
    }
}

#[test]
#[ignore]
fn bench_linear_u16_fasthq_paths() {
    let input = Path::new("test/2.jpeg");
    let decoded = decode::decode(input, false).unwrap();
    let cms = ColorPipeline::new(decoded.metadata.icc.as_deref()).unwrap();
    let target = crate::resize::compute_target_size(
        decoded.image.width,
        decoded.image.height,
        &ResizeOptions {
            width: None,
            height: None,
            max_width: None,
            max_height: None,
            max_side: Some(600),
            no_upscale: false,
            filter: None,
        },
    )
    .unwrap();
    let filter =
        crate::resize::resolve_filter(decoded.image.width, decoded.image.height, target, None);

    let mut image = decoded.image.clone();
    let tables = cms.linear_u16_tables().unwrap();
    cms.to_linear_u16_lut_in_place(&mut image, &tables).unwrap();
    image = crate::resize::resize(image, target, filter).unwrap();

    let data = match &image.data {
        WorkingData::Rgb16(data) => data,
        _ => panic!("expected RGB16 image"),
    };

    let downcast = crate::decode::WorkingImage {
        width: image.width,
        height: image.height,
        data: WorkingData::Rgb8(
            data.iter()
                .copied()
                .map(|v| ((v as u32 * 255 + 32767) / 65535) as u8)
                .collect(),
        ),
        color_space: image.color_space,
    };

    for _ in 0..3 {
        black_box(simpleyuv::working_image_to_yuv420(&image, &cms).unwrap());
        black_box(simpleyuv::linear_u16_to_yuv420(&image, &tables).unwrap());
        black_box(simpleyuv::working_image_to_yuv420(&downcast, &cms).unwrap());
        black_box(sharpyuv::working_image_to_yuv420(&image, true).unwrap());
    }

    let iterations = 20usize;

    let start = Instant::now();
    for _ in 0..iterations {
        black_box(simpleyuv::working_image_to_yuv420(&image, &cms).unwrap());
    }
    let old_simple_ms = start.elapsed().as_secs_f64() * 1000.0 / iterations as f64;

    let start = Instant::now();
    for _ in 0..iterations {
        black_box(simpleyuv::linear_u16_to_yuv420(&image, &tables).unwrap());
    }
    let sharpish_ms = start.elapsed().as_secs_f64() * 1000.0 / iterations as f64;

    let start = Instant::now();
    for _ in 0..iterations {
        black_box(simpleyuv::working_image_to_yuv420(&downcast, &cms).unwrap());
    }
    let downcast_ms = start.elapsed().as_secs_f64() * 1000.0 / iterations as f64;

    let start = Instant::now();
    for _ in 0..iterations {
        black_box(sharpyuv::working_image_to_yuv420(&image, true).unwrap());
    }
    let sharpyuv_ms = start.elapsed().as_secs_f64() * 1000.0 / iterations as f64;

    eprintln!(
        "linear-u16 paths on {}x{}: old_simple={:.3}ms sharpish={:.3}ms downcast_u8={:.3}ms sharpyuv={:.3}ms",
        image.width, image.height, old_simple_ms, sharpish_ms, downcast_ms, sharpyuv_ms
    );
}

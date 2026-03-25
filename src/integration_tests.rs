use std::fs::{self, File};
use std::path::Path;

use image::codecs::png::PngEncoder;
use image::metadata::Orientation;
use image::{ExtendedColorType, ImageEncoder, ImageFormat};
use lcms2::{CIExyY, Profile, ToneCurve};
use tempfile::tempdir;
use webpx::{get_exif, get_icc_profile};

use crate::cli::{Job, Mode, ResizeOptions};
use crate::decode::{self, WorkingData};
use crate::error::Error;
use crate::metadata::MetadataPolicy;
use crate::pipeline;

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
        fast: false,
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

fn read_webp(path: &Path) -> (u32, u32, Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>) {
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
    assert!(matches!(decoded_input.image.data, WorkingData::U8(_)));

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
fn fast_mode_produces_comparable_output_to_normal() {
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

    assert!(
        max_diff <= 2,
        "Fast mode should match normal mode (max diff: {max_diff})"
    );
}

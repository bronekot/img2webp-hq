use std::fs;

use img2webp_hq::{ConversionOptions, MetadataPolicy, convert, convert_file};
use tempfile::tempdir;

const JPEG: &[u8] = include_bytes!("../test/1.jpeg");

fn options() -> ConversionOptions {
    ConversionOptions::default()
        .with_quality(82.0)
        .with_max_side(600)
        .with_metadata(MetadataPolicy::None)
}

#[test]
fn converts_encoded_image_bytes_to_webp() {
    let webp = convert(JPEG, &options()).unwrap();

    assert!(webp.starts_with(b"RIFF"));
    assert_eq!(&webp[8..12], b"WEBP");
}

#[test]
fn file_adapter_matches_in_memory_api() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("input.jpeg");
    let output = dir.path().join("output.webp");
    fs::write(&input, JPEG).unwrap();

    convert_file(&input, &output, &options()).unwrap();

    let from_memory = convert(JPEG, &options()).unwrap();
    let from_file = fs::read(output).unwrap();
    assert_eq!(from_file, from_memory);
}

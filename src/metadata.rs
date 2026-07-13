use image::metadata::Orientation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetadataPolicy {
    None,
    All,
    Exif,
    Icc,
    Xmp,
}

impl MetadataPolicy {
    pub fn keep_icc(self) -> bool {
        matches!(self, Self::All | Self::Icc)
    }

    pub fn keep_exif(self) -> bool {
        matches!(self, Self::All | Self::Exif)
    }

    pub fn keep_xmp(self) -> bool {
        matches!(self, Self::All | Self::Xmp)
    }
}

#[derive(Debug, Clone)]
pub struct SourceMetadata {
    pub icc: Option<Vec<u8>>,
    pub exif: Option<Vec<u8>>,
    pub xmp: Option<Vec<u8>>,
    pub orientation: Orientation,
}

impl SourceMetadata {
    pub fn normalize_orientation(&mut self) {
        if let Some(exif) = &mut self.exif {
            let _ = Orientation::remove_from_exif_chunk(exif);
        }
    }
}

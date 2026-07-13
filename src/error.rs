use std::fmt;
use std::io;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Io(io::Error),
    Image(image::ImageError),
    InvalidArgument(String),
    Unsupported(String),
    Decode(String),
    Color(String),
    Encode(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidArgument(message.into())
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }

    pub fn decode(message: impl Into<String>) -> Self {
        Self::Decode(message.into())
    }

    pub fn color(message: impl Into<String>) -> Self {
        Self::Color(message.into())
    }

    pub fn encode(message: impl Into<String>) -> Self {
        Self::Encode(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Image(err) => write!(f, "{err}"),
            Self::InvalidArgument(message) => write!(f, "{message}"),
            Self::Unsupported(message) => write!(f, "{message}"),
            Self::Decode(message) => write!(f, "{message}"),
            Self::Color(message) => write!(f, "{message}"),
            Self::Encode(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Image(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<image::ImageError> for Error {
    fn from(value: image::ImageError) -> Self {
        Self::Image(value)
    }
}

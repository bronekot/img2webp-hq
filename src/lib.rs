mod cli;
mod cms;
mod decode;
mod encode;
mod error;
mod metadata;
mod pipeline;
mod resize;
mod sharpyuv;
mod simpleyuv;

#[cfg(test)]
mod integration_tests;

pub use cli::{Job, Mode, ResizeFilter, ResizeOptions};
pub use error::{Error, Result};
pub use metadata::MetadataPolicy;

pub fn run_from_env() -> Result<()> {
    let job = cli::parse_from_env()?;
    pipeline::run(job)
}

pub fn run(job: Job) -> Result<()> {
    pipeline::run(job)
}

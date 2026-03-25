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

pub use error::{Error, Result};

pub fn run_from_env() -> Result<()> {
    let job = cli::parse_from_env()?;
    pipeline::run(job)
}

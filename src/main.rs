use std::process::ExitCode;

fn main() -> ExitCode {
    match img2webp_hq::run_from_env() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

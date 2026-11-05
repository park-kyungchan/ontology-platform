//! One-shot local ProgramReport caller. This is not a human authentication route.
use ontd::program_pilot::{PilotError, TRUST_LIMIT};
use std::io::{Read, Write};
fn run() -> Result<String, &'static str> {
    if std::env::args_os().len() != 1 {
        return Err("invalid_request");
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| "pilot_input_failure")?;
    ontd::program_pilot::execute(&bytes).map_err(PilotError::code)
}
fn main() {
    match run() {
        Ok(answer) => {
            if writeln!(std::io::stdout().lock(), "{answer}").is_err() {
                eprintln!("pilot_output_failure@{}:{}", file!(), line!());
                std::process::exit(1);
            }
        }
        Err(code) => {
            eprintln!("{code}@{}:{}", file!(), line!());
            eprintln!("{TRUST_LIMIT}");
            std::process::exit(2);
        }
    }
}

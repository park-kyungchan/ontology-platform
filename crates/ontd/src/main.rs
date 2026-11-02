//! Explicitly synthetic stdin demonstration; no production credentials or paths.
use ont_core::{Error, Query, Response};
use std::io::{Read, Write};
const LIMIT: u64 = 65_536;
fn execute() -> Result<(String, i32), Error> {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--synthetic"] {
        return Err(Error::InvalidRequest);
    }
    let mut input = Vec::new();
    std::io::stdin()
        .take(LIMIT + 1)
        .read_to_end(&mut input)
        .map_err(|_| Error::StoreFailure)?;
    if input.len() as u64 > LIMIT {
        return Err(Error::InvalidRequest);
    }
    let query: Query = serde_json::from_slice(&input).map_err(|_| Error::InvalidRequest)?;
    let (mut service, session) = ontd::synthetic::setup()?;
    Ok((service.render(&session, query, ontd::synthetic::NOW)?, 0))
}
fn main() {
    let (line, status) = match execute() {
        Ok(result) => result,
        Err(e @ (Error::StoreFailure | Error::AuthorityFailure | Error::InvalidEvidence)) => {
            eprintln!("{}@{}:{}", e.code(), file!(), line!());
            std::process::exit(1);
        }
        Err(e) => match serde_json::to_string(&Response::refused(e)) {
            Ok(line) => (line, 2),
            Err(_) => {
                eprintln!("serialization_failure@{}:{}", file!(), line!());
                std::process::exit(1);
            }
        },
    };
    if writeln!(std::io::stdout().lock(), "{line}").is_err() {
        eprintln!("output_failure@{}:{}", file!(), line!());
        std::process::exit(1);
    }
    std::process::exit(status);
}

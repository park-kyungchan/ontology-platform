//! Explicitly synthetic stdin demonstration; no production credentials or paths.
use ont_core::{Error, Query, Response, answer::AnswerRecord};
use std::{
    ffi::OsString,
    io::{Read, Write},
};
const LIMIT: u64 = 65_536;
const HELP: &str = "Usage: ontd --synthetic [--answer-record]\n--answer-record: ontology-answer/v1; prose answers may only restate record fields.";
fn output_failure() -> ! {
    let _ = writeln!(
        std::io::stderr().lock(),
        "output_failure@crates/ontd/src/main.rs:37"
    );
    std::process::exit(1);
}
fn execute(args: &[OsString], answer_record: bool) -> Result<(String, i32), Error> {
    let valid = args == ["--synthetic"]
        || args == ["--synthetic", "--answer-record"]
        || args == ["--answer-record", "--synthetic"];
    if !valid {
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
    if answer_record {
        let (line, refused) =
            service.render_answer_record_with_status(&session, query, ontd::synthetic::NOW)?;
        Ok((line, if refused { 2 } else { 0 }))
    } else {
        Ok((service.render(&session, query, ontd::synthetic::NOW)?, 0))
    }
}
fn main() {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let args = argv.get(1..).unwrap_or_default();
    let non_utf8 = argv.iter().any(|arg| arg.to_str().is_none());
    if !non_utf8 && args == ["--help"] {
        if writeln!(std::io::stdout().lock(), "{HELP}").is_err() {
            output_failure();
        }
        return;
    }
    let answer_record = args.iter().any(|arg| arg == "--answer-record");
    // Lead-approved compatibility exception: default non-UTF-8 argv also
    // refuses without the legacy std::env::args panic and caller-byte echo.
    let result = if non_utf8 {
        Err(Error::InvalidRequest)
    } else {
        execute(args, answer_record)
    };
    let (line, status) = match result {
        Ok(result) => result,
        Err(e) if answer_record || non_utf8 => match AnswerRecord::from_error(e).render() {
            Ok(line) => (line, 2),
            Err(_) => std::process::exit(1),
        },
        Err(e @ (Error::StoreFailure | Error::AuthorityFailure | Error::InvalidEvidence)) => {
            // Diagnostic locations are part of the default CLI byte contract.
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}@crates/ontd/src/main.rs:25",
                e.code()
            );
            std::process::exit(1);
        }
        Err(e) => match serde_json::to_string(&Response::refused(e)) {
            Ok(line) => (line, 2),
            Err(_) => {
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "serialization_failure@crates/ontd/src/main.rs:31"
                );
                std::process::exit(1);
            }
        },
    };
    if writeln!(std::io::stdout().lock(), "{line}").is_err() {
        output_failure();
    }
    std::process::exit(status);
}

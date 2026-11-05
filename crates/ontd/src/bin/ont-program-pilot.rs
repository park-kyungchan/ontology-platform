//! One-shot local ProgramReport caller. This is not a human authentication route.
use ont_core::answer::{AnswerRecord, Version};
use ontd::program_pilot::{PilotError, TRUST_LIMIT};
use std::{
    ffi::OsString,
    io::{Read, Write},
};
const HELP: &str = "Usage: ont-program-pilot [--answer-record]\n--answer-record: ontology-answer/v1; prose answers may only restate record fields.\nOS-account principal only; this CLI does not authenticate a human.";
fn output_failure() -> ! {
    let _ = writeln!(
        std::io::stderr().lock(),
        "pilot_output_failure@crates/ontd/src/bin/ont-program-pilot.rs:19"
    );
    std::process::exit(1);
}
fn run(args: &[OsString], answer_record: Option<Version>) -> Result<(String, bool), &'static str> {
    if !(args.is_empty()
        || args.len() == 1
            && matches!(
                args[0].to_str(),
                Some("--answer-record" | "--answer-record=v1" | "--answer-record=v2")
            ))
    {
        return Err("invalid_request");
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| "pilot_input_failure")?;
    if let Some(version) = answer_record {
        ontd::program_pilot::execute_answer_record_version_with_status(&bytes, version)
    } else {
        ontd::program_pilot::execute(&bytes).map(|answer| (answer, false))
    }
    .map_err(PilotError::code)
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
    let answer_record = if args.iter().any(|arg| arg == "--answer-record=v2") {
        Some(Version::V2)
    } else if args
        .iter()
        .any(|arg| arg == "--answer-record" || arg == "--answer-record=v1")
    {
        Some(Version::V1)
    } else {
        None
    };
    let result = if answer_record.is_some() && non_utf8 {
        Err("invalid_request")
    } else {
        run(args, answer_record)
    };
    let (answer, status) = match result {
        Ok((answer, refused)) => (answer, if refused { 2 } else { 0 }),
        Err(code) if answer_record.is_some() => {
            match AnswerRecord::from_outcome_version(code, answer_record.unwrap_or(Version::V1))
                .render()
            {
                Ok(record) => (record, 2),
                Err(_) => std::process::exit(1),
            }
        }
        Err(code) => {
            // Preserve base stderr bytes when the new mode is not selected.
            if writeln!(
                std::io::stderr().lock(),
                "{code}@crates/ontd/src/bin/ont-program-pilot.rs:24\n{TRUST_LIMIT}"
            )
            .is_err()
            {
                std::process::exit(1);
            }
            std::process::exit(2);
        }
    };
    if writeln!(std::io::stdout().lock(), "{answer}").is_err() {
        output_failure();
    }
    std::process::exit(status);
}

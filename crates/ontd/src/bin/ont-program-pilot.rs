//! One-shot local ProgramReport caller. This is not a human authentication route.
use ont_core::answer::AnswerRecord;
use ontd::program_pilot::{PilotError, TRUST_LIMIT};
use std::io::{Read, Write};
const HELP: &str = "Usage: ont-program-pilot [--answer-record]\n--answer-record: ontology-answer/v1; prose answers may only restate record fields.\nOS-account principal only; this CLI does not authenticate a human.";
fn run(args: &[String], answer_record: bool) -> Result<(String, bool), &'static str> {
    if !args.is_empty() && args != ["--answer-record"] {
        return Err("invalid_request");
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| "pilot_input_failure")?;
    if answer_record {
        ontd::program_pilot::execute_answer_record_with_status(&bytes)
    } else {
        ontd::program_pilot::execute(&bytes).map(|answer| (answer, false))
    }
    .map_err(PilotError::code)
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--help"] {
        println!("{HELP}");
        return;
    }
    let answer_record = args.iter().any(|arg| arg == "--answer-record");
    let (answer, status) = match run(&args, answer_record) {
        Ok((answer, refused)) => (answer, if refused { 2 } else { 0 }),
        Err(code) if answer_record => match AnswerRecord::from_outcome(code).render() {
            Ok(record) => (record, 2),
            Err(_) => std::process::exit(1),
        },
        Err(code) => {
            eprintln!("{code}@{}:{}", file!(), line!());
            eprintln!("{TRUST_LIMIT}");
            std::process::exit(2);
        }
    };
    if writeln!(std::io::stdout().lock(), "{answer}").is_err() {
        eprintln!("pilot_output_failure@{}:{}", file!(), line!());
        std::process::exit(1);
    }
    std::process::exit(status);
}

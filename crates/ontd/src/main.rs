//! Explicitly synthetic stdin demonstration; no production credentials or paths.
use ont_core::{Error, Query, Response, answer::AnswerRecord};
use std::io::{Read, Write};
const LIMIT: u64 = 65_536;
const HELP: &str = "Usage: ontd --synthetic [--answer-record]\n--answer-record: ontology-answer/v1; prose answers may only restate record fields.";
fn execute(args: &[String], answer_record: bool) -> Result<(String, i32), Error> {
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--help"] {
        println!("{HELP}");
        return;
    }
    let answer_record = args.iter().any(|arg| arg == "--answer-record");
    let (line, status) = match execute(&args, answer_record) {
        Ok(result) => result,
        Err(e) if answer_record => match AnswerRecord::from_error(e).render() {
            Ok(line) => (line, 2),
            Err(_) => std::process::exit(1),
        },
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

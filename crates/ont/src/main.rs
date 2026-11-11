//! Thin binary over the `ont` library.
use std::io::{Read, Write};

fn uids() -> (u32, u32) {
    // /proc/self/status: "Uid: real, effective, saved, fs".
    let Ok(text) = std::fs::read_to_string("/proc/self/status") else {
        return (u32::MAX, u32::MAX);
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let fields: Vec<u32> = rest
                .split_whitespace()
                .filter_map(|f| f.parse().ok())
                .collect();
            if fields.len() >= 2 {
                return (fields[0], fields[1]);
            }
        }
    }
    (u32::MAX, u32::MAX)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stdin = Vec::new();
    // Only the commands that take a request read stdin (`--version`,
    // `schema` and `backup` must not wait on it).
    if args.iter().any(|a| a == "describe" || a == "get") {
        let _ = std::io::stdin()
            .take(ont::MAX_REQUEST as u64 + 65_537)
            .read_to_end(&mut stdin);
    }
    let (real, effective) = uids();
    match ont::run(&args, &stdin, real, effective) {
        Ok(outcome) => {
            if !outcome.stdout.is_empty() {
                let _ = write!(std::io::stdout().lock(), "{}", outcome.stdout);
            }
            if !outcome.stderr.is_empty() {
                let _ = write!(std::io::stderr().lock(), "{}", outcome.stderr);
            }
            std::process::exit(outcome.code);
        }
        Err(internal) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}@{}:{}",
                internal.0,
                file!(),
                line!()
            );
            std::process::exit(1);
        }
    }
}

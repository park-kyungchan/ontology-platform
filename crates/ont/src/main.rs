//! Thin binary over the `ont` library. Standard input is read by `run`, and
//! only for the commands that take a request.
use std::io::Write;

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
    let (real, effective) = uids();
    let outcome = ont::run(&args, std::io::stdin().lock(), real, effective);
    if !outcome.stdout.is_empty() {
        let _ = write!(std::io::stdout().lock(), "{}", outcome.stdout);
    }
    if !outcome.stderr.is_empty() {
        let _ = write!(std::io::stderr().lock(), "{}", outcome.stderr);
    }
    std::process::exit(outcome.code);
}

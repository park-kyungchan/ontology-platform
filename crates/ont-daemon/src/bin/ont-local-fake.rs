//! Standalone LOCAL FAKE demonstrator; cannot contact a network/source/account.
use ont_core::{Error, Response, action::PhysicalField};
use ont_daemon::{
    Service,
    local_fake::{self, ActionCommand},
};
use serde::Deserialize;
use std::{
    io::{Read, Write},
    path::PathBuf,
};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Init,
    Perturb {
        physical: PhysicalField,
        value: String,
    },
    Review {
        input: ont_core::action::ActionInput,
    },
    Dispatch {
        key: ont_core::action::RequestKey,
    },
    Inspect {
        key: ont_core::action::RequestKey,
    },
    Reconcile {
        key: ont_core::action::RequestKey,
        evidence: ont_store::actions::Evidence,
    },
}
fn execute() -> Result<String, Error> {
    let mut args = std::env::args().skip(1);
    let mut root = None;
    let mut profile = "normal".to_owned();
    let mut fault = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(Error::InvalidRequest)?;
        match flag.as_str() {
            "--root" if root.is_none() => root = Some(PathBuf::from(value)),
            "--profile" => profile = value,
            "--fault" if fault.is_none() => fault = Some(value),
            _ => return Err(Error::InvalidRequest),
        }
    }
    let root = root
        .filter(|p| p.is_absolute())
        .ok_or(Error::InvalidRequest)?;
    if fault.as_ref().is_some_and(|f| {
        ![
            "before_review_commit",
            "after_review_commit",
            "before_fence_commit",
            "after_fence_commit",
            "before_target_commit",
            "after_target_commit",
            "before_readback",
            "before_receipt_commit",
            "after_receipt_commit",
        ]
        .contains(&f.as_str())
    }) {
        return Err(Error::InvalidRequest);
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::StoreFailure)?;
    if bytes.len() > 65_536 {
        return Err(Error::InvalidRequest);
    }
    let command: Command = serde_json::from_slice(&bytes).map_err(|_| Error::InvalidRequest)?;
    if matches!(command, Command::Init) {
        local_fake::initialize(&root)?;
        return Ok("{\"status\":\"initialized\"}".into());
    }
    let (store, mut target) = local_fake::open(&root)?;
    if let Command::Perturb { physical, value } = &command {
        target.perturb_trusted(physical, value)?;
        return Ok("{\"status\":\"perturbed\"}".into());
    }
    let (authority, session) = local_fake::authority(&profile)?;
    let command = match command {
        Command::Review { input } => ActionCommand::Review { input },
        Command::Dispatch { key } => ActionCommand::Dispatch { key },
        Command::Inspect { key } => ActionCommand::Inspect { key },
        Command::Reconcile { key, evidence } => ActionCommand::Reconcile { key, evidence },
        _ => return Err(Error::InvalidRequest),
    };
    let mut hook = |boundary: ont_store::actions::Boundary| {
        if fault.as_deref() == Some(boundary.name()) {
            std::process::exit(boundary.exit_status());
        }
    };
    Service::new(store, authority).render_action(
        &session,
        &mut target,
        command,
        local_fake::NOW,
        &mut hook,
    )
}
fn main() {
    let (line, status) = match execute() {
        Ok(line) => (line, 0),
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

//! Read-only helper compiled against the EXACT accepted S06 source archive.
use ont_core::{Error, Id, Snapshot};
use ont_store::Store;
use std::path::Path;

fn read() -> Result<String, Error> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err(Error::InvalidRequest);
    }
    let id: Id = serde_json::from_str(&args[1]).map_err(|_| Error::InvalidRequest)?;
    let mut store = Store::open(Path::new(&args[0]))?;
    let object = store.snapshot(|s| s.get(&id))?;
    serde_json::to_string(&object).map_err(|_| Error::StoreFailure)
}
fn main() {
    match read() {
        Ok(raw) => println!("{raw}"),
        Err(e) => {
            eprintln!("{}@{}:{}", e.code(), file!(), line!());
            std::process::exit(1);
        }
    }
}

//! Refusals (exit 2) and internal errors (exit 1) of the operations.
use ont_store::Store;
use ont_store::schema_mode::StoreRefusal;
use serde_json::{Map, Value, json};
use std::path::Path;

/// An internal error: `code@file:line` of the place that raised it.
#[derive(Debug)]
pub struct Internal {
    pub code: &'static str,
    pub file: &'static str,
    pub line: u32,
}

/// Raise an [`Internal`] located at the macro's call site.
#[macro_export]
macro_rules! internal {
    ($code:expr) => {
        $crate::Internal {
            code: $code,
            file: file!(),
            line: line!(),
        }
    };
}

/// A refusal of input or admission: a code and extra answer fields.
#[derive(Debug)]
pub struct Refusal {
    pub code: &'static str,
    pub extra: Map<String, Value>,
}
impl Refusal {
    pub fn new(code: &'static str) -> Self {
        Self {
            code,
            extra: Map::new(),
        }
    }
    /// A field form/type refusal. Never attach the rejected value.
    pub fn input(code: &'static str, field: &str, rule: &str) -> Self {
        Self::new(code)
            .with("field", json!(field))
            .with("rule", json!(rule))
    }
    pub fn with(mut self, key: &str, value: Value) -> Self {
        self.extra.insert(key.to_string(), value);
        self
    }
}

/// How an operation fails.
#[derive(Debug)]
pub enum Failure {
    Refused(Refusal),
    Internal(Internal),
}
impl From<Refusal> for Failure {
    fn from(r: Refusal) -> Self {
        Self::Refused(r)
    }
}
impl From<Internal> for Failure {
    fn from(i: Internal) -> Self {
        Self::Internal(i)
    }
}

/// A store refusal becomes a refusal, except `store_failure` (a damaged or
/// foreign store), which is internal and located at the caller.
#[track_caller]
pub fn store_err(e: StoreRefusal) -> Failure {
    if e.code == "store_failure" {
        let at = std::panic::Location::caller();
        return Failure::Internal(Internal {
            code: "store_failure",
            file: at.file(),
            line: at.line(),
        });
    }
    if e.code == "store_path_is_symlink" {
        return Refusal::input(
            "store_path_is_symlink",
            "--store",
            "the store path, parent directories and adjacent files must not be symbolic links",
        )
        .into();
    }
    let mut refusal = Refusal::new(e.code);
    if let Some(v) = &e.violation {
        refusal = refusal.with("violation", json!(v));
    }
    Failure::Refused(refusal)
}

/// Open an existing schema store; format 3 migrates once to format 4.
/// Reads reopen read-only after migration; `index` keeps a writable handle.
/// A missing store is
/// `store_not_found` and carries the `path`.
#[track_caller]
pub fn open_store(path: &Path, writable: bool) -> Result<Store, Failure> {
    let opened = if writable {
        Store::open_schema_existing_writable(path)
    } else {
        Store::open_schema_existing(path)
    };
    match opened {
        Ok(store) => Ok(store),
        Err(e) => {
            let missing = e.code == "store_not_found";
            let failure = store_err(e);
            match failure {
                Failure::Refused(r) if missing => Err(Failure::Refused(
                    r.with("path", json!(path.display().to_string())),
                )),
                other => Err(other),
            }
        }
    }
}

/// `match` a store result, returning the located failure from the caller.
macro_rules! st {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return Err($crate::refusal::store_err(e)),
        }
    };
}
/// `match` an `std::io` result, returning an internal error with `code`.
macro_rules! io {
    ($e:expr, $code:expr) => {
        match $e {
            Ok(v) => v,
            Err(_) => return Err($crate::Failure::Internal($crate::internal!($code))),
        }
    };
}
pub(crate) use io;
pub(crate) use st;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn store_busy_is_a_refusal_and_store_failure_is_internal() {
        match store_err(StoreRefusal::busy()) {
            Failure::Refused(r) => assert_eq!(r.code, "store_busy"),
            Failure::Internal(i) => panic!("{i:?}"),
        }
        match store_err(StoreRefusal::new("store_failure")) {
            Failure::Internal(i) => {
                assert_eq!(i.code, "store_failure");
                assert!(i.file.ends_with("refusal.rs"), "{}", i.file);
            }
            Failure::Refused(r) => panic!("{r:?}"),
        }
    }
}

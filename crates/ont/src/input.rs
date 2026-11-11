//! Request size admission and the shared core JSON diagnostics.
use crate::Refusal;

pub(crate) fn pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}
pub(crate) fn parse<T: for<'de> serde::Deserialize<'de>>(bytes: &[u8]) -> Result<T, Refusal> {
    if bytes.len() > crate::MAX_REQUEST {
        return Err(Refusal::input(
            "invalid_request",
            "",
            "at most 65536 JSON bytes",
        ));
    }
    ont_core::input::parse_json(bytes)
        .map_err(|e| Refusal::input("invalid_request", &e.field, &e.rule))
}

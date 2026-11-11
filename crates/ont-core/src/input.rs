//! Strict JSON decoding with a field path and value-free diagnostic.
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub struct JsonInputError {
    /// JSON pointer; the empty string denotes the whole request.
    pub field: String,
    /// Expected input form, without the rejected value.
    pub rule: String,
}

/// Decode one complete JSON value. Only serde's expected form is exposed;
/// its unexpected-value text may contain private input and is discarded.
pub fn parse_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, JsonInputError> {
    let _: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| JsonInputError {
        field: String::new(),
        rule: "one valid JSON value".into(),
    })?;
    let mut de = serde_json::Deserializer::from_slice(bytes);
    serde_path_to_error::deserialize(&mut de).map_err(|e| {
        let mut field: String = e
            .path()
            .iter()
            .map(|segment| match segment {
                serde_path_to_error::Segment::Seq { index } => format!("/{index}"),
                serde_path_to_error::Segment::Map { key } => format!("/{}", escape(key)),
                serde_path_to_error::Segment::Enum { variant } => format!("/{}", escape(variant)),
                _ => String::new(),
            })
            .collect();
        let message = e.inner().to_string();
        let rule = if let Some(rest) = message.strip_prefix("missing field `") {
            field.push('/');
            field.push_str(&escape(rest.split('`').next().unwrap_or("")));
            "required field".into()
        } else if message.starts_with("unknown field `") {
            "field is not allowed in this object".into()
        } else if message.starts_with("duplicate field `") {
            "field must occur only once".into()
        } else if let Some((_, expected)) = message.rsplit_once(", expected ") {
            format!(
                "expected {}",
                expected.split(" at line ").next().unwrap_or(expected)
            )
        } else if let Some((_, expected)) = message.rsplit_once(", there are no ") {
            format!(
                "no {}",
                expected.split(" at line ").next().unwrap_or(expected)
            )
        } else {
            "value must match the request field's JSON form".into()
        };
        JsonInputError { field, rule }
    })
}
fn escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

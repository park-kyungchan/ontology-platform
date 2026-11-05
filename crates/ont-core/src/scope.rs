use crate::{Error, Namespace};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Exact authorization dimensions. Education retains its original wire shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Education {
        namespace: Namespace,
        class_key: String,
        student_key: String,
    },
    Program {
        namespace: Namespace,
        program_key: String,
    },
}

impl Scope {
    pub fn education(
        namespace: Namespace,
        class_key: impl Into<String>,
        student_key: impl Into<String>,
    ) -> Result<Self, Error> {
        let scope = Self::Education {
            namespace,
            class_key: class_key.into(),
            student_key: student_key.into(),
        };
        scope.validate()?;
        Ok(scope)
    }

    pub fn program(namespace: Namespace, program_key: impl Into<String>) -> Result<Self, Error> {
        let scope = Self::Program {
            namespace,
            program_key: program_key.into(),
        };
        scope.validate()?;
        Ok(scope)
    }

    pub fn namespace(&self) -> &Namespace {
        match self {
            Self::Education { namespace, .. } | Self::Program { namespace, .. } => namespace,
        }
    }

    /// Preserve exact UTF-8 and legacy nonempty scope keys. Namespace bounds
    /// match Id::validate; whitespace is identity data, not a normalization cue.
    pub fn validate(&self) -> Result<(), Error> {
        let namespace = self.namespace();
        if [&namespace.instance, &namespace.account]
            .iter()
            .any(|s| s.is_empty() || s.len() > 512)
        {
            return Err(Error::InvalidRequest);
        }
        let empty = match self {
            Self::Education {
                class_key,
                student_key,
                ..
            } => class_key.is_empty() || student_key.is_empty(),
            Self::Program { program_key, .. } => program_key.is_empty(),
        };
        if empty {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}

impl Serialize for Scope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("Scope", 3)?;
        state.serialize_field("namespace", self.namespace())?;
        match self {
            Self::Education {
                class_key,
                student_key,
                ..
            } => {
                state.serialize_field("class_key", class_key)?;
                state.serialize_field("student_key", student_key)?;
            }
            Self::Program { program_key, .. } => {
                state.serialize_field("domain", "program")?;
                state.serialize_field("program_key", program_key)?;
            }
        }
        state.end()
    }
}

const FIELDS: &[&str] = &[
    "namespace",
    "class_key",
    "student_key",
    "domain",
    "program_key",
];

// Derived struct readers also accept positional sequences. Scope's namespace
// wire contract requires an object; delegate its fields only after that check.
struct NamespaceMap(Namespace);
impl<'de> Deserialize<'de> for NamespaceMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NamespaceVisitor;
        impl<'de> Visitor<'de> for NamespaceVisitor {
            type Value = NamespaceMap;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a namespace object with instance and account")
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                Namespace::deserialize(de::value::MapAccessDeserializer::new(map)).map(NamespaceMap)
            }
        }
        deserializer.deserialize_map(NamespaceVisitor)
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ScopeVisitor;
        impl<'de> Visitor<'de> for ScopeVisitor {
            type Value = Scope;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an exact Education scope or tagged Program scope")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Scope, M::Error> {
                let mut namespace = None;
                let mut class_key = None;
                let mut student_key = None;
                let mut domain = None;
                let mut program_key = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "namespace" => {
                            if namespace.is_some() {
                                return Err(de::Error::duplicate_field("namespace"));
                            }
                            namespace = Some(map.next_value::<NamespaceMap>()?.0);
                        }
                        "class_key" => {
                            if class_key.is_some() {
                                return Err(de::Error::duplicate_field("class_key"));
                            }
                            class_key = Some(map.next_value::<String>()?);
                        }
                        "student_key" => {
                            if student_key.is_some() {
                                return Err(de::Error::duplicate_field("student_key"));
                            }
                            student_key = Some(map.next_value::<String>()?);
                        }
                        "domain" => {
                            if domain.is_some() {
                                return Err(de::Error::duplicate_field("domain"));
                            }
                            domain = Some(map.next_value::<String>()?);
                        }
                        "program_key" => {
                            if program_key.is_some() {
                                return Err(de::Error::duplicate_field("program_key"));
                            }
                            program_key = Some(map.next_value::<String>()?);
                        }
                        _ => return Err(de::Error::unknown_field(&key, FIELDS)),
                    }
                }
                let namespace = namespace.ok_or_else(|| de::Error::missing_field("namespace"))?;
                // Only the two complete key sets are valid. Never reinterpret a
                // malformed Program record through a legacy untagged fallback.
                match (class_key, student_key, domain, program_key) {
                    (Some(class), Some(student), None, None) => {
                        Scope::education(namespace, class, student).map_err(de::Error::custom)
                    }
                    (None, None, Some(domain), Some(program)) if domain == "program" => {
                        Scope::program(namespace, program).map_err(de::Error::custom)
                    }
                    _ => Err(de::Error::custom("invalid scope fields or domain")),
                }
            }
        }
        deserializer.deserialize_map(ScopeVisitor)
    }
}

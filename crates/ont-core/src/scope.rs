use crate::{Error, Namespace};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

pub(crate) const NAMESPACE_MAX_BYTES: usize = 512;
pub(crate) const PROGRAM_DOMAIN: &str = "program";

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
    Domain {
        namespace: Namespace,
        domain: String,
        keys: BTreeMap<String, String>,
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

    pub fn domain(
        namespace: Namespace,
        domain: impl Into<String>,
        keys: BTreeMap<String, String>,
    ) -> Result<Self, Error> {
        let scope = Self::Domain {
            namespace,
            domain: domain.into(),
            keys,
        };
        scope.validate()?;
        Ok(scope)
    }

    pub fn namespace(&self) -> &Namespace {
        match self {
            Self::Education { namespace, .. }
            | Self::Program { namespace, .. }
            | Self::Domain { namespace, .. } => namespace,
        }
    }

    /// Grant matching: `self` is the grant's scope. A `Domain` grant matches
    /// the same namespace and domain when every key it names equals the
    /// candidate's value; a key it omits matches any value. Other variants
    /// keep exact matching.
    pub fn matches(&self, candidate: &Self) -> bool {
        match (self, candidate) {
            (
                Self::Domain {
                    namespace,
                    domain,
                    keys,
                },
                Self::Domain {
                    namespace: other_namespace,
                    domain: other_domain,
                    keys: other_keys,
                },
            ) => {
                namespace == other_namespace
                    && domain == other_domain
                    && keys
                        .iter()
                        .all(|(k, v)| other_keys.get(k).is_some_and(|o| o == v))
            }
            _ => self == candidate,
        }
    }

    /// Preserve exact UTF-8 and legacy nonempty scope keys. Namespace bounds
    /// match Id::validate; whitespace is identity data, not a normalization cue.
    pub fn validate(&self) -> Result<(), Error> {
        let namespace = self.namespace();
        if [&namespace.instance, &namespace.account]
            .iter()
            .any(|s| s.is_empty() || s.len() > NAMESPACE_MAX_BYTES)
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
            Self::Domain { domain, keys, .. } => {
                domain.is_empty()
                    || domain == PROGRAM_DOMAIN
                    || keys.iter().any(|(k, v)| k.is_empty() || v.is_empty())
            }
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
            Self::Domain { domain, keys, .. } => {
                state.serialize_field("domain", domain)?;
                state.serialize_field("keys", keys)?;
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
    "keys",
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
                let mut keys = None;
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
                        "keys" => {
                            if keys.is_some() {
                                return Err(de::Error::duplicate_field("keys"));
                            }
                            keys = Some(map.next_value::<BTreeMap<String, String>>()?);
                        }
                        _ => return Err(de::Error::unknown_field(&key, FIELDS)),
                    }
                }
                let namespace = namespace.ok_or_else(|| de::Error::missing_field("namespace"))?;
                // Only the two complete key sets are valid. Never reinterpret a
                // malformed Program record through a legacy untagged fallback.
                match (class_key, student_key, domain, program_key, keys) {
                    (Some(class), Some(student), None, None, None) => {
                        Scope::education(namespace, class, student).map_err(de::Error::custom)
                    }
                    (None, None, Some(domain), Some(program), None) if domain == "program" => {
                        Scope::program(namespace, program).map_err(de::Error::custom)
                    }
                    (None, None, Some(domain), None, Some(keys)) if domain != "program" => {
                        Scope::domain(namespace, domain, keys).map_err(de::Error::custom)
                    }
                    _ => Err(de::Error::custom("invalid scope fields or domain")),
                }
            }
        }
        deserializer.deserialize_map(ScopeVisitor)
    }
}

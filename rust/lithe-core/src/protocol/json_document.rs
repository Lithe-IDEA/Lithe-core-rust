//! Strict JSON values shared by native-backed configuration editors.

use serde::{
    de::{MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value;
use std::{collections::BTreeMap, fmt, marker::PhantomData};

/// An ordered object that rejects duplicate keys at deserialization.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct UniqueMap<T>(pub(crate) BTreeMap<String, T>);

impl<T> Default for UniqueMap<T> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for UniqueMap<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for MapVisitor<T> {
            type Value = UniqueMap<T>;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an object with unique keys")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut input: A) -> Result<Self::Value, A::Error> {
                let mut entries = BTreeMap::new();
                while let Some((key, value)) = input.next_entry::<String, T>()? {
                    if entries.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate preference key"));
                    }
                }
                Ok(UniqueMap(entries))
            }
        }
        deserializer.deserialize_map(MapVisitor(PhantomData))
    }
}

/// A JSON value whose objects were all checked for duplicate keys by serde.
struct DuplicateCheckedValue(Value);

impl<'de> Deserialize<'de> for DuplicateCheckedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ValueVisitor;
        impl<'de> Visitor<'de> for ValueVisitor {
            type Value = DuplicateCheckedValue;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(DuplicateCheckedValue(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(DuplicateCheckedValue(value.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(DuplicateCheckedValue(value.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| DuplicateCheckedValue(Value::Number(number)))
                    .ok_or_else(|| E::custom("non-finite preference number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                self.visit_string(value.to_owned())
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(DuplicateCheckedValue(Value::String(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(DuplicateCheckedValue(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut input: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = input.next_element::<DuplicateCheckedValue>()? {
                    values.push(value.0);
                }
                Ok(DuplicateCheckedValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, input: A) -> Result<Self::Value, A::Error> {
                let entries = UniqueMap::<DuplicateCheckedValue>::deserialize(
                    serde::de::value::MapAccessDeserializer::new(input),
                )?;
                Ok(DuplicateCheckedValue(Value::Object(
                    entries
                        .0
                        .into_iter()
                        .map(|(key, value)| (key, value.0))
                        .collect(),
                )))
            }
        }
        deserializer.deserialize_any(ValueVisitor)
    }
}

/// Parses JSON without losing duplicate keys, including nested objects and
/// escaped spellings. Callers own byte budgets and schema-specific failures.
pub(crate) fn parse_unique_json(content: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str::<DuplicateCheckedValue>(content).map(|value| value.0)
}

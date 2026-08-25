//! Non-finite floats in a JSON-shaped model.
//!
//! JSON has no NaN or ±Infinity, and serde_json writes them as `null`
//! (`ser.rs`: `if !value.is_finite() { self.serialize_unit() }`) — which loses
//! the value *and*, for infinities, the sign. There is no option to change
//! that; `Number::from_f64` rejects non-finite outright.
//!
//! PlayMaker data does carry them: an `FsmAnimationCurve` uses infinite
//! tangents for stepped curves. So the model spells them out as the strings
//! JavaScript's `Number()` parses back — `"NaN"`, `"Infinity"`, `"-Infinity"`
//! — and leaves finite values as plain JSON numbers.

use serde::de::{self, Unexpected, Visitor};
use serde::{Deserializer, Serializer};

const NAN: &str = "NaN";
const INFINITY: &str = "Infinity";
const NEG_INFINITY: &str = "-Infinity";

fn serialize<S: Serializer>(value: f32, serializer: S) -> Result<S::Ok, S::Error> {
    if value.is_finite() {
        return serializer.serialize_f32(value);
    }
    serializer.serialize_str(if value.is_nan() {
        NAN
    } else if value > 0.0 {
        INFINITY
    } else {
        NEG_INFINITY
    })
}

fn parse(text: &str) -> Option<f32> {
    match text {
        NAN => Some(f32::NAN),
        INFINITY => Some(f32::INFINITY),
        NEG_INFINITY => Some(f32::NEG_INFINITY),
        _ => None,
    }
}

struct F32Visitor;

impl<'de> Visitor<'de> for F32Visitor {
    type Value = f32;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "a number, or \"NaN\" / \"Infinity\" / \"-Infinity\"")
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<f32, E> {
        Ok(v as f32)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<f32, E> {
        Ok(v as f32)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<f32, E> {
        Ok(v as f32)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<f32, E> {
        parse(v).ok_or_else(|| E::invalid_value(Unexpected::Str(v), &self))
    }
}

pub mod f32_field {
    use super::*;

    pub fn serialize<S: Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
        super::serialize(*value, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
        deserializer.deserialize_any(F32Visitor)
    }
}

pub mod f32_vec {
    use serde::ser::SerializeSeq;

    use super::*;

    pub fn serialize<S: Serializer>(values: &[f32], serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(values.len()))?;
        for value in values {
            seq.serialize_element(&Wrapper(*value))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<f32>, D::Error> {
        deserializer.deserialize_seq(SeqVisitor)
    }

    struct Wrapper(f32);

    impl serde::Serialize for Wrapper {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            super::serialize(self.0, serializer)
        }
    }

    struct Element(f32);

    impl<'de> serde::Deserialize<'de> for Element {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_any(F32Visitor).map(Element)
        }
    }

    struct SeqVisitor;

    impl<'de> Visitor<'de> for SeqVisitor {
        type Value = Vec<f32>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "a sequence of numbers")
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<f32>, A::Error> {
            let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(Element(v)) = seq.next_element()? {
                out.push(v);
            }
            Ok(out)
        }
    }
}

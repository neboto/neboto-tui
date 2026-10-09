//! JSON ⇄ CBOR for the Smithy RPC v2 CBOR protocol (CloudWatch).
//!
//! CBOR fixtures are authored as JSON, like the awsJson ones, and encoded
//! when the response goes out; requests are decoded back to JSON so a
//! fixture's `when` substrings can match them (`"AlarmName":"…"`).
//!
//! Two things JSON can't say on its own:
//! - **Timestamps** are CBOR tag 1 over epoch seconds, and the SDK rejects
//!   an untagged number. Write `{"$timestamp": {{epoch:now-2h}}}`.
//! - **Doubles vs integers**: the SDK reads a double only from a CBOR float,
//!   so a `Threshold` must be written `5.0`, not `5`. Integers stay `60`.

use minicbor::data::{IanaTag, Tag, Type};
use serde_json::{Map, Number, Value};

/// The key of the one-entry object that stands for a CBOR timestamp.
const TIMESTAMP_KEY: &str = "$timestamp";

/// Encode a JSON fixture body as CBOR. None if it isn't valid JSON.
pub fn from_json(text: &str) -> Option<Vec<u8>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let mut enc = minicbor::Encoder::new(Vec::new());
    encode(&value, &mut enc).ok()?;
    Some(enc.into_writer())
}

fn encode(
    value: &Value,
    enc: &mut minicbor::Encoder<Vec<u8>>,
) -> Result<(), minicbor::encode::Error<std::convert::Infallible>> {
    match value {
        Value::Null => {
            enc.null()?;
        }
        Value::Bool(b) => {
            enc.bool(*b)?;
        }
        Value::Number(n) => encode_number(n, enc)?,
        Value::String(s) => {
            enc.str(s)?;
        }
        Value::Array(items) => {
            enc.array(items.len() as u64)?;
            for item in items {
                encode(item, enc)?;
            }
        }
        Value::Object(map) => {
            if let (1, Some(Value::Number(secs))) = (map.len(), map.get(TIMESTAMP_KEY)) {
                enc.tag(IanaTag::Timestamp)?;
                return encode_number(secs, enc);
            }
            enc.map(map.len() as u64)?;
            for (k, v) in map {
                enc.str(k)?;
                encode(v, enc)?;
            }
        }
    }
    Ok(())
}

fn encode_number(
    n: &Number,
    enc: &mut minicbor::Encoder<Vec<u8>>,
) -> Result<(), minicbor::encode::Error<std::convert::Infallible>> {
    if let Some(i) = n.as_u64() {
        enc.u64(i)?;
    } else if let Some(i) = n.as_i64() {
        enc.i64(i)?;
    } else {
        enc.f64(n.as_f64().unwrap_or_default())?;
    }
    Ok(())
}

/// Decode a CBOR request body to compact JSON text, timestamps as
/// `{"$timestamp":…}` and byte strings as null. None if it isn't CBOR.
pub fn to_json(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return Some(String::new());
    }
    let mut dec = minicbor::Decoder::new(bytes);
    let value = decode(&mut dec).ok()?;
    serde_json::to_string(&value).ok()
}

fn decode(dec: &mut minicbor::Decoder) -> Result<Value, minicbor::decode::Error> {
    Ok(match dec.datatype()? {
        Type::Bool => Value::Bool(dec.bool()?),
        Type::Null | Type::Undefined => {
            dec.skip()?;
            Value::Null
        }
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => Value::from(dec.u64()?),
        Type::I8 | Type::I16 | Type::I32 | Type::I64 => Value::from(dec.i64()?),
        Type::F16 | Type::F32 | Type::F64 => {
            Number::from_f64(dec.f64()?).map(Value::Number).unwrap_or(Value::Null)
        }
        Type::String => Value::String(dec.str()?.to_string()),
        Type::StringIndef => {
            let mut s = String::new();
            for part in dec.str_iter()? {
                s.push_str(part?);
            }
            Value::String(s)
        }
        Type::Array | Type::ArrayIndef => {
            let len = dec.array()?;
            let mut items = Vec::new();
            loop {
                if len.is_some_and(|n| items.len() as u64 == n) {
                    break;
                }
                if len.is_none() && dec.datatype()? == Type::Break {
                    dec.skip()?;
                    break;
                }
                items.push(decode(dec)?);
            }
            Value::Array(items)
        }
        Type::Map | Type::MapIndef => {
            let len = dec.map()?;
            let mut map = Map::new();
            let mut seen = 0u64;
            loop {
                if len.is_some_and(|n| seen == n) {
                    break;
                }
                if len.is_none() && dec.datatype()? == Type::Break {
                    dec.skip()?;
                    break;
                }
                let key = match decode(dec)? {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                map.insert(key, decode(dec)?);
                seen += 1;
            }
            Value::Object(map)
        }
        Type::Tag => {
            let tag = dec.tag()?;
            let inner = decode(dec)?;
            if tag == Tag::from(IanaTag::Timestamp) {
                let mut map = Map::new();
                map.insert(TIMESTAMP_KEY.to_string(), inner);
                Value::Object(map)
            } else {
                inner
            }
        }
        _ => {
            dec.skip()?;
            Value::Null
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_cbor() {
        let json = r#"{"Name":"a","Count":60,"Threshold":5.0,"On":true,"At":{"$timestamp":1700000000},"List":["x"],"Empty":[]}"#;
        let bytes = from_json(json).unwrap();
        assert_eq!(to_json(&bytes).unwrap(), json);
    }

    #[test]
    fn timestamps_are_tag_1_and_doubles_are_floats() {
        let bytes = from_json(r#"{"$timestamp":1700000000}"#).unwrap();
        assert_eq!(bytes[0], 0xc1, "tag 1");
        let bytes = from_json("5.0").unwrap();
        assert_eq!(bytes[0], 0xfb, "f64");
        let bytes = from_json("5").unwrap();
        assert_eq!(bytes, vec![0x05], "small uint");
    }

    #[test]
    fn empty_object_is_an_empty_map() {
        assert_eq!(from_json("{}").unwrap(), vec![0xa0]);
    }
}

//! Lossless integers at the JavaScript boundary only. Song files and domain
//! serializers keep their native numeric representation.
use serde_json::{json, Value};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const INTEGER_TAG: &str = "$crestU64";

/// Tag unsigned integers that JSON.parse would round. All other values retain
/// their shape, including strings that happen to contain decimal digits.
pub fn encode(mut value: Value) -> Value {
    match &mut value {
        Value::Number(number) => {
            if let Some(integer) = number.as_u64().filter(|n| *n > MAX_SAFE_INTEGER) {
                return json!({INTEGER_TAG: integer.to_string()});
            }
        }
        Value::Array(values) => {
            for child in values {
                *child = encode(child.take());
            }
        }
        Value::Object(fields) => {
            for child in fields.values_mut() {
                *child = encode(child.take());
            }
        }
        _ => {}
    }
    value
}

/// Decode explicit integer tags before ordinary typed command validation.
/// Reject untagged unsafe numbers: their original value is already unknowable.
pub fn decode(mut value: Value) -> Result<Value, &'static str> {
    match &mut value {
        Value::Object(fields) if fields.get(INTEGER_TAG).is_some_and(Value::is_string) => {
            let text = fields[INTEGER_TAG]
                .as_str()
                .ok_or("Invalid exact integer")?;
            if fields.len() != 1
                || text.is_empty()
                || !text.bytes().all(|byte| byte.is_ascii_digit())
                || (text.len() > 1 && text.starts_with('0'))
            {
                return Err("Invalid exact integer");
            }
            let integer = text
                .parse::<u64>()
                .map_err(|_| "Exact integer exceeds u64")?;
            return Ok(json!(integer));
        }
        Value::Object(fields) => {
            for child in fields.values_mut() {
                *child = decode(child.take())?;
            }
        }
        Value::Array(values) => {
            for child in values {
                *child = decode(child.take())?;
            }
        }
        Value::Number(number)
            if number
                .as_f64()
                .is_some_and(|n| n.abs() > MAX_SAFE_INTEGER as f64) =>
        {
            return Err("Unsafe numeric input; use an exact integer");
        }
        _ => {}
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_tracker::time::Beat;

    #[test]
    fn preserves_nested_times_identities_seeds_and_numeric_text() {
        let source = json!({"id":MAX_SAFE_INTEGER + 2,"seed":u64::MAX,
            "name":"9007199254740993","small":MAX_SAFE_INTEGER,
            "events":[{"at":Beat::new(MAX_SAFE_INTEGER + 2, 36_028_797_018_963_968).unwrap()}]});
        let wire = encode(source.clone());
        assert_eq!(wire["id"][INTEGER_TAG], "9007199254740993");
        assert_eq!(wire["seed"][INTEGER_TAG], u64::MAX.to_string());
        assert_eq!(wire["small"], MAX_SAFE_INTEGER);
        assert_eq!(wire["name"], source["name"]);
        assert_eq!(decode(wire).unwrap(), source);
    }

    #[test]
    fn malformed_and_already_rounded_numbers_are_rejected() {
        for value in [
            json!({INTEGER_TAG:"18446744073709551616"}),
            json!({INTEGER_TAG:"-1"}),
            json!({INTEGER_TAG:"1.0"}),
            json!({INTEGER_TAG:"01"}),
            json!({INTEGER_TAG:"+1"}),
            json!({INTEGER_TAG:""}),
            json!({INTEGER_TAG:"1","extra":true}),
            json!({"at":{"numerator":MAX_SAFE_INTEGER + 1,"denominator":1}}),
        ] {
            assert!(decode(value).is_err());
        }
        let invalid_beat = decode(json!({"numerator":{INTEGER_TAG:"1"},"denominator":0})).unwrap();
        assert!(serde_json::from_value::<Beat>(invalid_beat).is_err());
        // Strings are never implicitly coerced into numbers.
        assert!(serde_json::from_value::<u64>(decode(json!("9007199254740993")).unwrap()).is_err());
    }
}

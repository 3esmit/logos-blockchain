pub mod genesis;
use serde_norway::Value;

/// Deep-merge `overwrite` into `input`. Mappings are merged recursively;
/// any other type is replaced wholesale by the overwrite value.
#[must_use]
pub fn overwrite_yaml(input: Value, overwrite: Value) -> Value {
    match (input, overwrite) {
        (Value::Mapping(mut input_map), Value::Mapping(overwrite_map)) => {
            for (key, overwrite_value) in overwrite_map {
                input_map
                    .entry(key)
                    .and_modify(|input_value| {
                        *input_value = overwrite_yaml(input_value.clone(), overwrite_value.clone());
                    })
                    .or_insert(overwrite_value);
            }
            Value::Mapping(input_map)
        }
        (_, overwrite) => overwrite,
    }
}

#[expect(
    clippy::too_long_first_doc_paragraph,
    reason = "Necessary documentation"
)]
/// Convert a dot-notation `"some.nested.key=value"` string into a nested
/// [`serde_norway::Value`] mapping. The value portion is parsed as YAML so
/// integers, booleans, quoted strings, etc. are typed correctly.
///
/// # Errors
///
/// Returns an error string when no `=` separator is found or when the value
/// portion is not valid YAML.
pub fn value_from_dotted_kv(s: &str) -> Result<Value, String> {
    let (key_path, raw_value) = s
        .split_once('=')
        .ok_or_else(|| format!("missing '=' separator in override: {s}"))?;

    let leaf: Value = serde_norway::from_str(raw_value)
        .map_err(|e| format!("invalid YAML value '{raw_value}': {e}"))?;

    // Wrap the leaf in nested mappings, innermost key first (right-to-left fold).
    let nested = key_path.split('.').rev().fold(leaf, |acc, key| {
        let mut map = serde_norway::Mapping::new();
        map.insert(Value::String(key.to_owned()), acc);
        Value::Mapping(map)
    });

    Ok(nested)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_kv_single_key() {
        let v = value_from_dotted_kv("foo=bar").unwrap();
        let expected: Value = serde_norway::from_str("foo: bar").unwrap();
        assert_eq!(v, expected);
    }

    #[test]
    fn dotted_kv_nested() {
        let v = value_from_dotted_kv("a.b.c=42").unwrap();
        let expected: Value = serde_norway::from_str("a:\n  b:\n    c: 42").unwrap();
        assert_eq!(v, expected);
    }

    #[test]
    fn dotted_kv_missing_eq() {
        assert!(value_from_dotted_kv("no-separator").is_err());
    }

    #[test]
    fn overwrite_yaml_merges_nested() {
        let base: Value = serde_norway::from_str("a:\n  x: 1\n  y: 2").unwrap();
        let patch: Value = serde_norway::from_str("a:\n  y: 99\n  z: 3").unwrap();
        let result = overwrite_yaml(base, patch);
        let expected: Value = serde_norway::from_str("a:\n  x: 1\n  y: 99\n  z: 3").unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn dotted_overrides_preserve_yaml_scalar_and_collection_types() {
        for raw in [
            "true",
            "null",
            "18446744073709551615",
            "-9223372036854775808",
            "\"001\"",
            "[1, false, null]",
            "{nested: 42}",
            "!Custom tagged",
        ] {
            let value = value_from_dotted_kv(&format!("outer.leaf={raw}")).unwrap();
            let expected: Value = serde_norway::from_str(raw).unwrap();
            assert_eq!(value["outer"]["leaf"], expected, "override: {raw}");
        }
        assert!(value_from_dotted_kv("outer.leaf=[unterminated").is_err());
    }

    #[test]
    fn overwrite_replaces_sequences_and_preserves_unmodified_tags() {
        let base = serde_norway::from_str("list: [1, 2]\nkeep: !Custom data\n").unwrap();
        let patch = serde_norway::from_str("list: [3]\n").unwrap();
        let result = overwrite_yaml(base, patch);
        let expected: Value = serde_norway::from_str("list: [3]\nkeep: !Custom data\n").unwrap();
        assert_eq!(result, expected);
        let encoded = serde_norway::to_string(&result).unwrap();
        assert_eq!(serde_norway::from_str::<Value>(&encoded).unwrap(), expected);
    }
}

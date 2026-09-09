//! Portable execution identity; entropy allocation belongs to the native runtime.
use std::{fmt, str::FromStr};

/// An opaque 128-bit identifier, serialized as exactly 32 lowercase hexadecimal
/// digits. Ordering is for indexing only and does not imply execution chronology.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExecutionId([u8; 16]);

impl ExecutionId {
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
    /// Construct an explicit identity, useful for deterministic fixtures/imports.
    /// Live runs must use a source that guarantees independent allocation.
    pub const fn from_u128(value: u128) -> Self {
        Self(value.to_be_bytes())
    }
}

impl fmt::Display for ExecutionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidExecutionId;
impl fmt::Display for InvalidExecutionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("execution ID must contain exactly 32 lowercase hexadecimal digits")
    }
}
impl std::error::Error for InvalidExecutionId {}
impl FromStr for ExecutionId {
    type Err = InvalidExecutionId;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 32
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(InvalidExecutionId);
        }
        let mut bytes = [0; 16];
        for (byte, pair) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
            let digit = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
            *byte = digit(pair[0]) * 16 + digit(pair[1]);
        }
        Ok(Self(bytes))
    }
}
impl serde::Serialize for ExecutionId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
impl<'de> serde::Deserialize<'de> for ExecutionId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_roundtrips_all_bits_as_canonical_text() {
        for value in [
            0,
            1,
            u64::MAX as u128 + 1,
            0xabcdef01234567899876543210fedcba,
            u128::MAX,
        ] {
            let id = ExecutionId::from_u128(value);
            let text = format!("{value:032x}");
            assert_eq!(id.to_string(), text);
            assert_eq!(text.parse::<ExecutionId>().unwrap(), id);
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(json, format!("\"{text}\""));
            assert_eq!(serde_json::from_str::<ExecutionId>(&json).unwrap(), id);
        }
    }

    #[test]
    fn rejects_noncanonical_and_legacy_numeric_ids() {
        for text in [
            "",
            "1",
            "0000000000000000000000000000000",
            "000000000000000000000000000000000",
            "ABCDEF01234567899876543210FEDCBA",
            "abcdef01-2345-6789-9876-543210fedcba",
            "g0000000000000000000000000000000",
            "é000000000000000000000000000000",
            " 0000000000000000000000000000000",
        ] {
            assert!(text.parse::<ExecutionId>().is_err(), "{text}");
            assert!(serde_json::from_value::<ExecutionId>(serde_json::json!(text)).is_err());
        }
        for json in ["1", "null", "true", "[]", "{}"] {
            assert!(serde_json::from_str::<ExecutionId>(json).is_err(), "{json}");
        }
    }
}

//! Source identity and source mapping primitives shared by every layer.

use std::fmt;

pub use rowan::{TextRange, TextSize};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileId(u32);

impl FileId {
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceRevision([u8; 32]);

impl SourceRevision {
    pub fn of(source: &str) -> Self {
        Self(*blake3::hash(source.as_bytes()).as_bytes())
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for SourceRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SourceRevision(")?;
        for byte in &self.0[..6] {
            write!(f, "{byte:02x}")?;
        }
        write!(f, "…)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DocumentVersion(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SyntaxOrigin {
    pub file: FileId,
    pub range: TextRange,
}

impl Serialize for SyntaxOrigin {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct SerializableOrigin {
            file: FileId,
            start: u32,
            end: u32,
        }
        SerializableOrigin {
            file: self.file,
            start: self.range.start().into(),
            end: self.range.end().into(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SyntaxOrigin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct SerializableOrigin {
            file: FileId,
            start: u32,
            end: u32,
        }
        let value = SerializableOrigin::deserialize(deserializer)?;
        if value.start > value.end {
            return Err(serde::de::Error::custom("source range start exceeds end"));
        }
        Ok(Self {
            file: value.file,
            range: TextRange::new(TextSize::from(value.start), TextSize::from(value.end)),
        })
    }
}

impl SyntaxOrigin {
    pub const fn new(file: FileId, range: TextRange) -> Self {
        Self { file, range }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_are_content_addressed() {
        assert_eq!(SourceRevision::of("same"), SourceRevision::of("same"));
        assert_ne!(SourceRevision::of("same"), SourceRevision::of("different"));
    }
}

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

    /// Length-delimited hashing for canonical semantic inputs without staging
    /// a large serialized workspace document.
    pub fn of_parts<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> Self {
        let mut hash = blake3::Hasher::new();
        hash.update(b"webtest-source-parts/v1");
        for part in parts {
            hash.update(&(part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        Self(*hash.finalize().as_bytes())
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

/// A revision-bound source link. Hosts supply paths; this DTO performs no I/O.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    pub path: String,
    pub origin: SyntaxOrigin,
    pub revision: SourceRevision,
}

/// Length-delimited semantic identity hashing shared by lowering and portable plans.
pub fn semantic_key(parent: &[u8; 32], kind: &str, name: &str, ordinal: u32) -> [u8; 32] {
    let parent = parent
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    *SourceRevision::of(&format!(
        "webtest-semantic/v1/{parent}/{kind}/{}/{name}/{ordinal}",
        name.len()
    ))
    .as_bytes()
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

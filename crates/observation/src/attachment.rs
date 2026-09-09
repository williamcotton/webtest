//! Immutable references to evidence whose persistence was acknowledged.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Screenshot,
    DomSnapshot,
    Evidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub path: PathBuf,
}

/// Digest and length describe the acknowledged bytes. A reader must verify the
/// referenced file before trusting it; trace export supplies its own safe path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    #[serde(flatten)]
    pub artifact: Artifact,
    pub byte_length: u64,
    pub blake3: [u8; 32],
}

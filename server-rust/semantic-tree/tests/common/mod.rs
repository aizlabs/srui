//! Shared loader for the §32 conformance suite manifest.
//!
//! Implements: §32 (conformance suites).
//!
//! `protocol/conformance-vectors/suites/manifest.json` is the single index of the twelve
//! suites. Vector discovery goes through [`suite_vectors`] rather than a bare directory scan
//! so that a suite's fixture count is pinned exactly, the way
//! `server-rust/semantic-tree/build.rs` pins the registry entry counts. A floor assertion
//! (`len() >= n`) would let a reorganization silently drop fixtures and still report green.

#![allow(dead_code)]

pub mod fixture_replay;

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub suites: Vec<Suite>,
}

#[derive(Debug, Deserialize)]
pub struct Suite {
    pub id: u32,
    pub slug: String,
    pub name: String,
    pub spec_sections: Vec<String>,
    pub status: String,
    #[serde(default)]
    pub vectors: Option<SuiteVectors>,
    #[serde(default)]
    pub gaps: Vec<SuiteGap>,
}

#[derive(Debug, Deserialize)]
pub struct SuiteVectors {
    pub dir: String,
    /// Exact number of JSON vectors expected in `dir`. Absent for code-driven suites.
    #[serde(default)]
    pub count: Option<usize>,
    /// Generated fixture file names expected in `dir`.
    #[serde(default)]
    pub generated: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SuiteGap {
    pub scenario: String,
    pub reason: String,
    pub future_task: String,
}

/// Absolute path of `protocol/conformance-vectors/`.
pub fn vectors_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/conformance-vectors")
}

pub fn load_manifest() -> Manifest {
    let path = vectors_root().join("suites/manifest.json");
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read conformance manifest {:?}: {}", path, e));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("Failed to parse conformance manifest {:?}: {}", path, e))
}

pub fn suite(manifest: &Manifest, id: u32) -> &Suite {
    manifest
        .suites
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("Conformance manifest has no suite with id {}", id))
}

/// Sorted `.json` vectors for `suite_id`, asserted to match the manifest count exactly.
///
/// Both directions matter: a missing file means coverage was lost in a move, and an extra
/// file means a fixture was added without being declared, so no runner accounts for it.
pub fn suite_vectors(suite_id: u32) -> Vec<PathBuf> {
    let manifest = load_manifest();
    let suite = suite(&manifest, suite_id);
    let vectors = suite
        .vectors
        .as_ref()
        .unwrap_or_else(|| panic!("Suite {} declares no vectors in the manifest", suite_id));
    let expected = vectors.count.unwrap_or_else(|| {
        panic!(
            "Suite {} declares no exact vector count in the manifest",
            suite_id
        )
    });

    let dir = vectors_root().join(&vectors.dir);
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| {
            panic!(
                "Failed to read suite {} vector dir {:?}: {}",
                suite_id, dir, e
            )
        })
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("Failed to read directory entry in {:?}: {}", dir, e))
                .path()
        })
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    files.sort();

    assert_eq!(
        files.len(),
        expected,
        "Suite {} ({}) declares exactly {} vectors in the manifest but {:?} contains {}. \
         Update protocol/conformance-vectors/suites/manifest.json in the same commit that \
         adds or removes a fixture.",
        suite_id,
        suite.slug,
        expected,
        dir,
        files.len()
    );
    files
}

/// Absolute path of a generated fixture declared by `suite_id`.
pub fn suite_generated(suite_id: u32, file_name: &str) -> PathBuf {
    let manifest = load_manifest();
    let suite = suite(&manifest, suite_id);
    let vectors = suite
        .vectors
        .as_ref()
        .unwrap_or_else(|| panic!("Suite {} declares no vectors in the manifest", suite_id));
    assert!(
        vectors.generated.iter().any(|g| g == file_name),
        "Suite {} does not declare generated fixture '{}' in the manifest",
        suite_id,
        file_name
    );
    vectors_root().join(&vectors.dir).join(file_name)
}

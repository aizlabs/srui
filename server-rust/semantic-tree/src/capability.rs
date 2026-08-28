//! Semantic capability profiles, sets, and negotiation logic (§4 inv. 13, §6.1, §6.4, §11, §15).
//!
//! # Architecture & Protocol Invariants
//!
//! - **§4 Invariant 13**: Unknown required semantics fail explicitly; optional semantics are
//!   negotiated or have documented fallbacks.
//! - **§6.1 Core Objects**: `Capability` represents a negotiated feature/profile identifier and version.
//! - **§6.4 Extension Namespaces**: Capabilities use canonical URIs with versions (e.g. `org.srui.standard-widgets/1`,
//!   `org.srui.terminal/1`, `org.srui.richtext/1`, `org.srui.vector-scene/1`).
//! - **§15 Capability Negotiation**: Handshake negotiation evaluates client-offered profiles against
//!   server required and optional profile lists, producing the active negotiated set.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

/// Canonical standard and extension profile identifiers (§6.4, §7, §9, §11, §21, §30).
pub const PROFILE_STANDARD_WIDGETS: &str = "org.srui.standard-widgets";
pub const PROFILE_TERMINAL: &str = "org.srui.terminal";
pub const PROFILE_RICHTEXT: &str = "org.srui.richtext";
pub const PROFILE_VECTOR_SCENE: &str = "org.srui.vector-scene";
pub const PROFILE_MEDIA_SURFACE: &str = "org.srui.media-surface";
pub const PROFILE_CODING: &str = "org.srui.coding";

/// Strongly-typed capability profile identifier with a major version (§6.4, §15).
///
/// Formatted on the wire and in configuration as `"<name>/<version>"`, for example:
/// `"org.srui.standard-widgets/1"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Profile {
    name: String,
    version: u32,
}

impl Profile {
    /// Constructs a new `Profile` with name and major version.
    pub fn new(name: impl Into<String>, version: u32) -> Result<Self, ParseProfileError> {
        let name = name.into();
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(ParseProfileError::EmptyName);
        }
        if version == 0 {
            return Err(ParseProfileError::InvalidVersion("version must be >= 1".to_string()));
        }
        Ok(Self {
            name: trimmed.to_string(),
            version,
        })
    }

    /// Constructs a `Profile` from static strings and constants without runtime parsing checks.
    pub fn from_static(name: &'static str, version: u32) -> Self {
        Self {
            name: name.to_string(),
            version,
        }
    }

    /// Returns the profile's canonical name (without version suffix).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the profile's major version number.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Parses a profile identifier string formatted as `"<name>/<version>"`.
    pub fn parse(s: &str) -> Result<Self, ParseProfileError> {
        s.parse()
    }

    /// Standard Widget Profile v1 (`org.srui.standard-widgets/1`, §7).
    pub fn standard_widgets_v1() -> Self {
        Self::from_static(PROFILE_STANDARD_WIDGETS, 1)
    }

    /// Terminal Compatibility Extension Profile v1 (`org.srui.terminal/1`, §21).
    pub fn terminal_v1() -> Self {
        Self::from_static(PROFILE_TERMINAL, 1)
    }

    /// RichText Profile v1 (`org.srui.richtext/1`, §9).
    pub fn richtext_v1() -> Self {
        Self::from_static(PROFILE_RICHTEXT, 1)
    }

    /// VectorScene Retained Graphics Profile v1 (`org.srui.vector-scene/1`, §11.2).
    pub fn vector_scene_v1() -> Self {
        Self::from_static(PROFILE_VECTOR_SCENE, 1)
    }

    /// MediaSurface Video/Surface Streaming Profile v1 (`org.srui.media-surface/1`, §5.2).
    pub fn media_surface_v1() -> Self {
        Self::from_static(PROFILE_MEDIA_SURFACE, 1)
    }

    /// Coding Domain Extension Profile v1 (`org.srui.coding/1`, §30).
    pub fn coding_v1() -> Self {
        Self::from_static(PROFILE_CODING, 1)
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.name, self.version)
    }
}

impl FromStr for Profile {
    type Err = ParseProfileError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ParseProfileError::EmptyString);
        }
        let (name, version_str) = s
            .rsplit_once('/')
            .ok_or_else(|| ParseProfileError::MissingVersionDelimiter(s.to_string()))?;

        let name = name.trim();
        if name.is_empty() {
            return Err(ParseProfileError::EmptyName);
        }

        let version: u32 = version_str
            .trim()
            .parse()
            .map_err(|_| ParseProfileError::InvalidVersion(version_str.to_string()))?;

        if version == 0 {
            return Err(ParseProfileError::InvalidVersion("version must be >= 1".to_string()));
        }

        Ok(Self {
            name: name.to_string(),
            version,
        })
    }
}

impl TryFrom<&str> for Profile {
    type Error = ParseProfileError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl TryFrom<String> for Profile {
    type Error = ParseProfileError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// Error returned when parsing a capability profile string fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseProfileError {
    /// String is empty or contains only whitespace.
    EmptyString,
    /// Profile name portion before the version delimiter is empty.
    EmptyName,
    /// Missing `'/'` delimiter separating profile name and version number.
    MissingVersionDelimiter(String),
    /// Version portion is not a valid positive integer.
    InvalidVersion(String),
}

impl fmt::Display for ParseProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyString => write!(f, "profile string is empty"),
            Self::EmptyName => write!(f, "profile name is empty"),
            Self::MissingVersionDelimiter(s) => write!(
                f,
                "missing version delimiter '/' in profile string: {:?}",
                s
            ),
            Self::InvalidVersion(v) => {
                write!(f, "invalid profile version number: {:?}", v)
            }
        }
    }
}

impl std::error::Error for ParseProfileError {}

/// Set of capability profiles negotiated between client and server (§15).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CapabilitySet {
    profiles: BTreeSet<Profile>,
}

impl CapabilitySet {
    /// Constructs a new empty `CapabilitySet`.
    pub fn new() -> Self {
        Self {
            profiles: BTreeSet::new(),
        }
    }

    /// Constructs a `CapabilitySet` by parsing a slice of profile strings.
    pub fn from_str_slice(slice: &[&str]) -> Result<Self, ParseProfileError> {
        let mut set = Self::new();
        for &s in slice {
            set.insert(Profile::parse(s)?);
        }
        Ok(set)
    }

    /// Inserts a profile into the set, returning `true` if it was newly inserted.
    pub fn insert(&mut self, profile: Profile) -> bool {
        self.profiles.insert(profile)
    }

    /// Parses and inserts a profile string into the set.
    pub fn insert_str(&mut self, s: &str) -> Result<bool, ParseProfileError> {
        let profile = Profile::parse(s)?;
        Ok(self.insert(profile))
    }

    /// Removes a profile from the set, returning `true` if it was present.
    pub fn remove(&mut self, profile: &Profile) -> bool {
        self.profiles.remove(profile)
    }

    /// Returns `true` if the set contains the exact specified profile and version.
    pub fn contains(&self, profile: &Profile) -> bool {
        self.profiles.contains(profile)
    }

    /// Returns `true` if the set contains the exact profile matching the string.
    pub fn contains_str(&self, s: &str) -> bool {
        if let Ok(profile) = Profile::parse(s) {
            self.contains(&profile)
        } else {
            false
        }
    }

    /// Returns `true` if the set contains any version of the profile with the given name.
    pub fn contains_name(&self, name: &str) -> bool {
        self.profiles.iter().any(|p| p.name() == name)
    }

    /// Returns the first `Profile` matching the given name, if present.
    pub fn get_profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name() == name)
    }

    /// Returns the version number of the profile with the given name, if present.
    pub fn get_version(&self, name: &str) -> Option<u32> {
        self.get_profile(name).map(|p| p.version())
    }

    /// Returns an iterator over the profiles in deterministic lexicographical order.
    pub fn iter(&self) -> impl Iterator<Item = &Profile> {
        self.profiles.iter()
    }

    /// Returns the number of profiles in the set.
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// Returns `true` if the set contains no profiles.
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// Returns `true` if this set contains all profiles present in `other`.
    pub fn is_superset(&self, other: &CapabilitySet) -> bool {
        other.profiles.is_subset(&self.profiles)
    }

    /// Returns `true` if all profiles in this set are present in `other`.
    pub fn is_subset(&self, other: &CapabilitySet) -> bool {
        self.profiles.is_subset(&other.profiles)
    }

    /// Computes the intersection of two capability sets.
    pub fn intersection(&self, other: &CapabilitySet) -> Self {
        Self {
            profiles: self.profiles.intersection(&other.profiles).cloned().collect(),
        }
    }

    /// Computes the union of two capability sets.
    pub fn union(&self, other: &CapabilitySet) -> Self {
        Self {
            profiles: self.profiles.union(&other.profiles).cloned().collect(),
        }
    }

    /// Computes the difference (`self - other`) of two capability sets.
    pub fn difference(&self, other: &CapabilitySet) -> Self {
        Self {
            profiles: self.profiles.difference(&other.profiles).cloned().collect(),
        }
    }

    /// Returns a vector of the formatted profile strings in this set.
    pub fn to_string_vec(&self) -> Vec<String> {
        self.profiles.iter().map(|p| p.to_string()).collect()
    }

    /// Computes the negotiated capability set between client-offered profiles and server requirements (§15).
    ///
    /// # Invariants & Matching Rules
    ///
    /// - **§4 Invariant 13**: Unknown required semantics must fail explicitly. If any profile in `server_required`
    ///   is not present in `client_offered`, negotiation fails with [`NegotiationError::UnsatisfiedRequiredProfiles`].
    /// - **Optional Profiles**: If an optional profile in `server_optional` is offered by the client, it is enabled
    ///   in the negotiated set. If not offered by the client, it is omitted without error.
    /// - **Unknown Client Profiles**: If the client offers profiles not requested or supported by the server,
    ///   they are ignored and omitted from the negotiated set.
    pub fn negotiate(
        client_offered: &CapabilitySet,
        server_required: &CapabilitySet,
        server_optional: &CapabilitySet,
    ) -> Result<CapabilitySet, NegotiationError> {
        // 1. §4 Invariant 13: Verify all required profiles are offered by the client
        let mut missing_required = Vec::new();
        for req in server_required.iter() {
            if !client_offered.contains(req) {
                missing_required.push(req.clone());
            }
        }

        if !missing_required.is_empty() {
            return Err(NegotiationError::UnsatisfiedRequiredProfiles {
                missing: missing_required,
            });
        }

        // 2. Build active negotiated set: all required profiles + matching optional profiles
        let mut negotiated = CapabilitySet::new();
        for req in server_required.iter() {
            negotiated.insert(req.clone());
        }
        for opt in server_optional.iter() {
            if client_offered.contains(opt) {
                negotiated.insert(opt.clone());
            }
        }

        Ok(negotiated)
    }

    /// Convenience instance method: negotiates this client capability set against server requirements.
    pub fn negotiate_with(
        &self,
        server_required: &CapabilitySet,
        server_optional: &CapabilitySet,
    ) -> Result<CapabilitySet, NegotiationError> {
        Self::negotiate(self, server_required, server_optional)
    }
}

impl FromIterator<Profile> for CapabilitySet {
    fn from_iter<T: IntoIterator<Item = Profile>>(iter: T) -> Self {
        Self {
            profiles: iter.into_iter().collect(),
        }
    }
}

impl fmt::Display for CapabilitySet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CapabilitySet{:?}", self.to_string_vec())
    }
}

/// Server-side capability configuration specifying required and optional profiles (§15).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerCapabilities {
    /// Profiles that MUST be supported by the client for the session to proceed (§4 inv. 13, §15).
    pub required: CapabilitySet,
    /// Profiles that MAY be negotiated if supported by the client (§15).
    pub optional: CapabilitySet,
}

impl ServerCapabilities {
    /// Constructs a new `ServerCapabilities` specification.
    pub fn new(required: CapabilitySet, optional: CapabilitySet) -> Self {
        Self { required, optional }
    }

    /// Computes the negotiated capability set for a connecting client's offered profiles (§15).
    pub fn negotiate(&self, client_offered: &CapabilitySet) -> Result<CapabilitySet, NegotiationError> {
        CapabilitySet::negotiate(client_offered, &self.required, &self.optional)
    }
}

/// Error returned when capability negotiation fails (§4 invariant 13, §15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiationError {
    /// One or more required profiles (§15) were not offered by the client.
    UnsatisfiedRequiredProfiles {
        missing: Vec<Profile>,
    },
}

impl fmt::Display for NegotiationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsatisfiedRequiredProfiles { missing } => {
                write!(
                    f,
                    "unsatisfied required capabilities: client did not offer required profile(s): {:?}",
                    missing.iter().map(|p| p.to_string()).collect::<Vec<_>>()
                )
            }
        }
    }
}

impl std::error::Error for NegotiationError {}

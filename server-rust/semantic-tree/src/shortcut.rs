//! Platform-neutral semantic keyboard shortcuts: standard property 31 `shortcut`.
//!
//! Implements: §4 inv. 13 (unknown required semantics fail explicitly), §7.2/§7.4 (a common
//! semantic property on admitted standard widgets), §7.6/§7.7 (a shortcut on a `Button` or
//! `Toggle` is the same semantic event a click produces; there is no shortcut event type),
//! §22.5 (a shortcut on an editor or collection moves local focus only and sends nothing) and
//! §26 (a bounded, closed grammar checked on every decode and apply path).
//!
//! The value is a string in the closed canonical grammar
//!
//! ```text
//! shortcut = [ "primary+" ] [ "shift+" ] [ "alt+" ] key        ; at most 32 bytes
//! key      = "a".."z" | "0".."9"
//!          | "return" | "escape" | "tab" | "space" | "delete"
//!          | "up" | "down" | "left" | "right" | "home" | "end" | "page_up" | "page_down"
//!          | "f1".."f12"
//! ```
//!
//! `primary` is Command on macOS and Control elsewhere; mapping it to a native key equivalent is
//! the renderer's job. Every key except `f1`–`f12` requires `primary`, so a bare letter, digit or
//! editing key never competes with a text editor, a collection or the key-view loop. Exactly one
//! spelling is valid for each combination, so two shortcuts conflict exactly when their strings
//! are equal. Tokens are split on the `+` byte and compared byte for byte. The Swift replica (`client-macos/SemanticModel/Shortcut.swift`) implements the same
//! grammar and admission rules with the same error codes.

use crate::ids::TypeRef;
use std::fmt;

/// Maximum encoded length of a `shortcut` value in bytes (§26). The longest valid value,
/// `primary+shift+alt+page_down`, is 27 bytes; the bound is checked before any parsing.
pub const MAX_SHORTCUT_BYTES: usize = 32;

/// A named, non-character key admitted by the grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    Return,
    Escape,
    Tab,
    Space,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// Function key `f1`–`f12`.
    Function(u8),
}

impl NamedKey {
    const TABLE: [(&'static str, NamedKey); 13] = [
        ("return", NamedKey::Return),
        ("escape", NamedKey::Escape),
        ("tab", NamedKey::Tab),
        ("space", NamedKey::Space),
        ("delete", NamedKey::Delete),
        ("up", NamedKey::Up),
        ("down", NamedKey::Down),
        ("left", NamedKey::Left),
        ("right", NamedKey::Right),
        ("home", NamedKey::Home),
        ("end", NamedKey::End),
        ("page_up", NamedKey::PageUp),
        ("page_down", NamedKey::PageDown),
    ];

    fn parse(token: &str) -> Option<Self> {
        if let Some((_, key)) = Self::TABLE.iter().find(|(name, _)| *name == token) {
            return Some(*key);
        }
        let digits = token.strip_prefix('f')?;
        // Canonical only: "f1".."f12", never "f01" or "f0".
        if digits.is_empty()
            || digits.starts_with('0')
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let n: u8 = digits.parse().ok()?;
        (1..=12).contains(&n).then_some(NamedKey::Function(n))
    }

    /// Canonical token for this key.
    pub fn token(self) -> String {
        match self {
            NamedKey::Function(n) => format!("f{n}"),
            other => Self::TABLE
                .iter()
                .find(|(_, key)| *key == other)
                .map(|(name, _)| (*name).to_string())
                .expect("every non-function named key is in the table"),
        }
    }
}

/// The key of a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutKey {
    /// A lowercase ASCII letter `a`–`z` or digit `0`–`9`.
    Character(char),
    Named(NamedKey),
}

impl ShortcutKey {
    /// Whether the key may be bound without `primary` (only `f1`–`f12`).
    pub fn is_function_key(self) -> bool {
        matches!(self, ShortcutKey::Named(NamedKey::Function(_)))
    }
}

/// A validated canonical shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub primary: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: ShortcutKey,
}

/// Why a `shortcut` value is not in the canonical grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutError {
    /// The value is not a string.
    NotAString,
    /// The value is empty.
    Empty,
    /// The value exceeds [`MAX_SHORTCUT_BYTES`].
    TooLong { length: usize },
    /// A token is neither a modifier nor an admitted key (includes empty tokens and case variants).
    UnknownToken(String),
    /// A modifier appears more than once.
    DuplicateModifier(&'static str),
    /// Modifiers are not in the canonical `primary`, `shift`, `alt` order.
    MisorderedModifier(&'static str),
    /// Only modifiers were given.
    MissingKey,
    /// A key other than `f1`–`f12` was given without `primary`.
    RequiresPrimary(String),
}

impl fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAString => write!(f, "a shortcut must be a string"),
            Self::Empty => write!(f, "a shortcut must not be empty"),
            Self::TooLong { length } => write!(
                f,
                "a shortcut is at most {MAX_SHORTCUT_BYTES} bytes, got {length}"
            ),
            Self::UnknownToken(token) => {
                write!(f, "unknown shortcut token \"{}\"", escape_token(token))
            }
            Self::DuplicateModifier(m) => write!(f, "modifier {m:?} appears more than once"),
            Self::MisorderedModifier(m) => write!(
                f,
                "modifier {m:?} is out of the canonical primary, shift, alt order"
            ),
            Self::MissingKey => write!(f, "a shortcut must end in a key"),
            Self::RequiresPrimary(key) => write!(
                f,
                "key \"{}\" requires the primary modifier; only f1-f12 may be bare",
                escape_token(key)
            ),
        }
    }
}

impl std::error::Error for ShortcutError {}

/// Renders a token for an error message: printable ASCII as itself, `"` and `\` backslash-escaped,
/// every other Unicode scalar as `\u{hex}`. The Swift replica's `escapedShortcutToken` is
/// identical, so diagnostics read the same in both languages (Rust's `{:?}` escaping is not
/// reproducible in Swift, so it is not used).
pub fn escape_token(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    for c in token.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c),
            _ => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
        }
    }
    out
}

const MODIFIERS: [&str; 3] = ["primary", "shift", "alt"];

impl Shortcut {
    /// Parses a value in the closed canonical grammar, failing explicitly otherwise (§4 inv. 13).
    pub fn parse(value: &str) -> Result<Self, ShortcutError> {
        if value.len() > MAX_SHORTCUT_BYTES {
            return Err(ShortcutError::TooLong {
                length: value.len(),
            });
        }
        if value.is_empty() {
            return Err(ShortcutError::Empty);
        }
        let tokens: Vec<&str> = value.split('+').collect();
        let (key_token, modifier_tokens) = tokens.split_last().expect("split yields one token");
        let mut seen = [false; 3];
        let mut last_rank: Option<usize> = None;
        for token in modifier_tokens {
            let Some(rank) = MODIFIERS.iter().position(|m| m == token) else {
                return Err(ShortcutError::UnknownToken((*token).to_string()));
            };
            if seen[rank] {
                return Err(ShortcutError::DuplicateModifier(MODIFIERS[rank]));
            }
            if last_rank.is_some_and(|last| rank < last) {
                return Err(ShortcutError::MisorderedModifier(MODIFIERS[rank]));
            }
            seen[rank] = true;
            last_rank = Some(rank);
        }
        if MODIFIERS.contains(key_token) {
            let rank = MODIFIERS
                .iter()
                .position(|m| m == key_token)
                .expect("contained");
            if seen[rank] {
                return Err(ShortcutError::DuplicateModifier(MODIFIERS[rank]));
            }
            return Err(ShortcutError::MissingKey);
        }
        let key = parse_key(key_token)
            .ok_or_else(|| ShortcutError::UnknownToken((*key_token).to_string()))?;
        let shortcut = Shortcut {
            primary: seen[0],
            shift: seen[1],
            alt: seen[2],
            key,
        };
        if !shortcut.primary && !key.is_function_key() {
            return Err(ShortcutError::RequiresPrimary((*key_token).to_string()));
        }
        Ok(shortcut)
    }

    /// The canonical spelling; `Shortcut::parse(s)?.canonical() == s` for every valid `s`.
    pub fn canonical(&self) -> String {
        let mut out = String::new();
        for (enabled, name) in [
            (self.primary, MODIFIERS[0]),
            (self.shift, MODIFIERS[1]),
            (self.alt, MODIFIERS[2]),
        ] {
            if enabled {
                out.push_str(name);
                out.push('+');
            }
        }
        match self.key {
            ShortcutKey::Character(c) => out.push(c),
            ShortcutKey::Named(named) => out.push_str(&named.token()),
        }
        out
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

impl From<Shortcut> for String {
    fn from(shortcut: Shortcut) -> Self {
        shortcut.canonical()
    }
}

impl std::str::FromStr for Shortcut {
    type Err = ShortcutError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

fn parse_key(token: &str) -> Option<ShortcutKey> {
    let bytes = token.as_bytes();
    if bytes.len() == 1 && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
        return Some(ShortcutKey::Character(bytes[0] as char));
    }
    NamedKey::parse(token).map(ShortcutKey::Named)
}

/// What a shortcut does on an admitted node type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutBehavior {
    /// `Button`: the client sends `ACTIVATE` exactly as a click does (§7.6).
    Activate,
    /// `Toggle`: the client flips the value and sends `VALUE_CHANGED` exactly as a click does.
    Toggle,
    /// `TextInput`, `TextArea`, `List`, `Table`, `Tree`, `RichText`: local focus only, no event
    /// (§22.5).
    Focus,
}

/// The behavior of a shortcut on `node_type`, or `None` when the type does not admit one.
pub fn shortcut_behavior(node_type: TypeRef) -> Option<ShortcutBehavior> {
    match node_type {
        TypeRef::BUTTON => Some(ShortcutBehavior::Activate),
        TypeRef::TOGGLE => Some(ShortcutBehavior::Toggle),
        TypeRef::TEXT_INPUT
        | TypeRef::TEXT_AREA
        | TypeRef::LIST
        | TypeRef::TABLE
        | TypeRef::TREE
        | TypeRef::RICH_TEXT => Some(ShortcutBehavior::Focus),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_values_round_trip() {
        for value in [
            "primary+r",
            "primary+shift+r",
            "primary+alt+r",
            "primary+shift+alt+r",
            "primary+0",
            "primary+9",
            "primary+return",
            "primary+shift+alt+page_down",
            "f1",
            "f12",
            "shift+f5",
            "alt+f3",
            "primary+f10",
        ] {
            let parsed = Shortcut::parse(value).unwrap_or_else(|e| panic!("{value}: {e}"));
            assert_eq!(parsed.canonical(), value);
        }
    }

    #[test]
    fn non_canonical_values_fail_with_their_reason() {
        let cases: [(&str, ShortcutError); 17] = [
            ("", ShortcutError::Empty),
            (
                "primary+shift+alt+page_down_xxxxx",
                ShortcutError::TooLong { length: 33 },
            ),
            ("cmd+r", ShortcutError::UnknownToken("cmd".into())),
            ("primary+R", ShortcutError::UnknownToken("R".into())),
            ("Primary+r", ShortcutError::UnknownToken("Primary".into())),
            ("primary+", ShortcutError::UnknownToken("".into())),
            ("+r", ShortcutError::UnknownToken("".into())),
            ("primary++r", ShortcutError::UnknownToken("".into())),
            ("primary+f13", ShortcutError::UnknownToken("f13".into())),
            ("primary+f01", ShortcutError::UnknownToken("f01".into())),
            ("primary+comma", ShortcutError::UnknownToken("comma".into())),
            (
                "primary+primary+r",
                ShortcutError::DuplicateModifier("primary"),
            ),
            (
                "primary+shift+shift",
                ShortcutError::DuplicateModifier("shift"),
            ),
            (
                "shift+primary+r",
                ShortcutError::MisorderedModifier("primary"),
            ),
            ("primary+shift", ShortcutError::MissingKey),
            ("r", ShortcutError::RequiresPrimary("r".into())),
            (
                "shift+return",
                ShortcutError::RequiresPrimary("return".into()),
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(Shortcut::parse(value), Err(expected), "{value}");
        }
    }

    #[test]
    fn the_length_bound_is_checked_before_parsing() {
        let long = "x".repeat(MAX_SHORTCUT_BYTES + 1);
        assert_eq!(
            Shortcut::parse(&long),
            Err(ShortcutError::TooLong {
                length: MAX_SHORTCUT_BYTES + 1
            })
        );
        // Exactly at the bound the value is parsed (and here refused for its content).
        let at_bound = "y".repeat(MAX_SHORTCUT_BYTES);
        assert_eq!(
            Shortcut::parse(&at_bound),
            Err(ShortcutError::UnknownToken(at_bound.clone()))
        );
    }

    #[test]
    fn admission_is_closed() {
        assert_eq!(
            shortcut_behavior(TypeRef::BUTTON),
            Some(ShortcutBehavior::Activate)
        );
        assert_eq!(
            shortcut_behavior(TypeRef::TOGGLE),
            Some(ShortcutBehavior::Toggle)
        );
        for focus in [
            TypeRef::TEXT_INPUT,
            TypeRef::TEXT_AREA,
            TypeRef::LIST,
            TypeRef::TABLE,
            TypeRef::TREE,
            TypeRef::RICH_TEXT,
        ] {
            assert_eq!(shortcut_behavior(focus), Some(ShortcutBehavior::Focus));
        }
        for refused in [
            TypeRef::SURFACE,
            TypeRef::TEXT,
            TypeRef::SELECT,
            TypeRef::SLIDER,
            TypeRef::MENU,
            TypeRef::standard(999),
        ] {
            assert_eq!(shortcut_behavior(refused), None);
        }
    }
}

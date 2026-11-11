//! Privacy budget for the observation port (contract C07).
//!
//! The port is disabled by default and has no content or environment fields.
//! Enabled callers must supply opaque, non-secret correlation ids. Size and path
//! validation applies to the segment and every link; syntax checks cannot tell a
//! secret from an otherwise valid opaque id.

use serde::{Deserialize, Serialize};

use super::correlation::{CorrelationField, CorrelationIds};
use super::segment::ObservationSegmentRecord;

/// Why one correlation id value was refused.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyViolationKind {
    /// Empty, or longer than [`PrivacyBudget::max_id_bytes`].
    Oversized,
    /// Contains a character outside printable ASCII, so it cannot be an opaque
    /// id. This is what rejects transcripts, newline-bearing text, and
    /// environment dumps.
    Unprintable,
    /// Looks like an absolute, home-anchored, or drive-qualified path, so it
    /// would disclose a private filesystem location.
    PrivatePath,
}

/// One refused value. The offending value itself is never retained.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum PrivacyViolation {
    /// A correlation id slot was refused.
    Id {
        field: CorrelationField,
        kind: PrivacyViolationKind,
    },
    /// A span carried more predecessor/queue links than the budget allows.
    TooManyLinks,
}

impl PrivacyViolation {
    /// The refused correlation slot, when the violation names one.
    pub const fn field(self) -> Option<CorrelationField> {
        match self {
            Self::Id { field, .. } => Some(field),
            Self::TooManyLinks => None,
        }
    }

    /// The refusal reason.
    pub const fn kind(self) -> Option<PrivacyViolationKind> {
        match self {
            Self::Id { kind, .. } => Some(kind),
            Self::TooManyLinks => None,
        }
    }
}

/// Bounds the observation port will accept before refusing a record.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrivacyBudget {
    /// Longest accepted correlation id, in bytes.
    pub max_id_bytes: usize,
    /// Most span links one segment may carry.
    pub max_links: usize,
}

impl PrivacyBudget {
    /// Default budget: 128-byte ids, at most 8 links per segment.
    pub const STANDARD: Self = Self {
        max_id_bytes: 128,
        max_links: 8,
    };

    /// Validates one correlation id value without retaining it.
    pub fn id_violation(&self, value: &str) -> Option<PrivacyViolationKind> {
        if value.is_empty() || value.len() > self.max_id_bytes {
            return Some(PrivacyViolationKind::Oversized);
        }
        if !value.chars().all(|character| character.is_ascii_graphic()) {
            return Some(PrivacyViolationKind::Unprintable);
        }
        if looks_like_private_path(value) {
            return Some(PrivacyViolationKind::PrivatePath);
        }
        None
    }

    /// Returns the first correlation slot this budget refuses.
    ///
    /// Iteration follows contract order, so the answer is deterministic.
    pub fn correlation_violation(&self, ids: &CorrelationIds) -> Option<PrivacyViolation> {
        ids.iter().find_map(|(field, value)| {
            self.id_violation(value)
                .map(|kind| PrivacyViolation::Id { field, kind })
        })
    }

    /// Applies the same budget to every causal context, before buffering or logging.
    pub(super) fn record_violation(
        &self,
        record: &ObservationSegmentRecord,
    ) -> Option<PrivacyViolation> {
        self.correlation_violation(&record.correlation)
            .or_else(|| {
                (record.links.len() > self.max_links).then_some(PrivacyViolation::TooManyLinks)
            })
            .or_else(|| {
                record
                    .links
                    .iter()
                    .find_map(|link| self.correlation_violation(&link.correlation))
            })
    }
}

impl Default for PrivacyBudget {
    fn default() -> Self {
        Self::STANDARD
    }
}

/// True when the value discloses a private filesystem location.
///
/// Repository-relative positions such as `crates/x.rs:12` stay acceptable; only
/// absolute (`/`), home-anchored (`~`), UNC (`\\`), and drive-qualified (`C:\`)
/// forms are refused.
fn looks_like_private_path(value: &str) -> bool {
    if value.starts_with('/') || value.starts_with('~') || value.starts_with("\\\\") {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 3 && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/')
}

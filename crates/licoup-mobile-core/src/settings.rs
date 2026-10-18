//! The resource and settings facts the mobile entry reads from its own owner.
//!
//! The mobile client has no settings store of its own. What it has is the
//! bounded client resource policy — history pages, search results, state
//! quotas — and the privately written client state root, and both already
//! belong to `licoup-client-state`. This module is the mobile entry's read
//! path to them, so the surface has one place that answers "what may this
//! client hold" instead of each caller picking its own numbers.
//!
//! The state root is resolved through the shared portable data root, which the
//! platform bridge points at the app's own files directory before it
//! dispatches. Nothing here creates a directory or a file.

use std::path::PathBuf;

use licoup_client_state::{ClientResourceBounds, ClientResourcePolicy, paths};

/// The mobile entry's settings and resource facts.
#[derive(Clone, Copy, Debug)]
pub struct MobileSettings {
    policy: ClientResourcePolicy,
}

impl Default for MobileSettings {
    fn default() -> Self {
        Self::standard()
    }
}

impl MobileSettings {
    /// The client's standard bounded policy. The mobile entry takes the same
    /// policy the desktop client does; it does not enlarge a bound because the
    /// viewport is smaller.
    #[must_use]
    pub fn standard() -> Self {
        Self {
            policy: ClientResourcePolicy::standard(),
        }
    }

    /// The same settings over a caller-supplied policy.
    #[must_use]
    pub fn with_policy(policy: ClientResourcePolicy) -> Self {
        Self { policy }
    }

    #[must_use]
    pub fn policy(self) -> ClientResourcePolicy {
        self.policy
    }

    #[must_use]
    pub fn bounds(self) -> ClientResourceBounds {
        self.policy.bounds()
    }

    /// The privately written client state root under the portable data root.
    ///
    /// It is resolved, never created: a root that cannot be resolved is an
    /// error the caller reports, not a directory this call invents.
    pub fn state_root(self) -> anyhow::Result<PathBuf> {
        paths::portable_state_root()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mobile_entry_takes_the_standard_bounded_policy() {
        let settings = MobileSettings::standard();
        assert_eq!(
            settings.bounds().history_page_size,
            ClientResourcePolicy::standard().bounds().history_page_size
        );
        assert!(!settings.policy().allows_unbounded_collections());
        assert_eq!(MobileSettings::default().bounds(), settings.bounds());
    }
}

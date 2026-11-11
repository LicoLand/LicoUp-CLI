//! Dual-namespace local search. Provider history and protected communication
//! share cursor/failure semantics and never share a content domain.

use super::policy::{CapacityFailure, ClientResourcePolicy};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchNamespace {
    ProviderHistory,
    ProtectedCommunication,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchCursor {
    namespace: SearchNamespace,
    keyset: u64,
}

impl SearchCursor {
    pub const fn origin(namespace: SearchNamespace) -> Self {
        Self {
            namespace,
            keyset: 0,
        }
    }

    pub const fn namespace(self) -> SearchNamespace {
        self.namespace
    }

    pub const fn keyset(self) -> u64 {
        self.keyset
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchHit {
    pub identity: String,
    pub rank: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub next: Option<SearchCursor>,
    pub total: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchError {
    Capacity(CapacityFailure),
    AuthorizationRequired,
    NamespaceMismatch,
}

impl SearchError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Capacity(failure) => failure.code(),
            Self::AuthorizationRequired => "authorization_required",
            Self::NamespaceMismatch => "search_namespace_mismatch",
        }
    }
}

/// One authority, two isolated namespaces. Protected communication stays
/// `authorization_required` until an exact Protocol Line is admitted.
pub struct LocalSearchAuthority {
    policy: ClientResourcePolicy,
    provider_hits: Vec<SearchHit>,
}

impl LocalSearchAuthority {
    pub fn new(policy: ClientResourcePolicy) -> Self {
        Self {
            policy,
            provider_hits: Vec::new(),
        }
    }

    pub fn index_provider_identity(
        &mut self,
        identity: impl Into<String>,
    ) -> Result<(), SearchError> {
        if self.provider_hits.len() as u32 >= self.policy.search_page_size().saturating_mul(32) {
            return Err(SearchError::Capacity(CapacityFailure::QuotaExceeded {
                class: super::policy::ResourceClass::SearchResult,
            }));
        }
        self.provider_hits.push(SearchHit {
            identity: identity.into(),
            rank: self.provider_hits.len() as u32,
        });
        Ok(())
    }

    pub fn query(
        &self,
        namespace: SearchNamespace,
        cursor: SearchCursor,
    ) -> Result<SearchPage, SearchError> {
        if cursor.namespace() != namespace {
            return Err(SearchError::NamespaceMismatch);
        }
        match namespace {
            SearchNamespace::ProtectedCommunication => Err(SearchError::AuthorizationRequired),
            SearchNamespace::ProviderHistory => {
                let page_size = self.policy.search_page_size() as usize;
                let start = cursor.keyset() as usize;
                if start > self.provider_hits.len() {
                    return Err(SearchError::Capacity(CapacityFailure::CursorInvalid));
                }
                let end = (start + page_size).min(self.provider_hits.len());
                let hits = self.provider_hits[start..end].to_vec();
                let next = if end < self.provider_hits.len() {
                    Some(SearchCursor {
                        namespace,
                        keyset: end as u64,
                    })
                } else {
                    None
                };
                Ok(SearchPage {
                    hits,
                    next,
                    total: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::resource_bounds::policy::ClientResourcePolicy;

    #[test]
    fn namespaces_do_not_share_content_and_protected_stays_unsealed() {
        let mut search = LocalSearchAuthority::new(ClientResourcePolicy::default_bounded());
        search
            .index_provider_identity("synthetic-provider-1")
            .expect("index");
        let page = search
            .query(
                SearchNamespace::ProviderHistory,
                SearchCursor::origin(SearchNamespace::ProviderHistory),
            )
            .expect("query");
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.total, None);
        assert_eq!(
            search
                .query(
                    SearchNamespace::ProtectedCommunication,
                    SearchCursor::origin(SearchNamespace::ProtectedCommunication),
                )
                .expect_err("blocked")
                .code(),
            "authorization_required"
        );
    }

    #[test]
    fn cursor_from_the_other_namespace_is_rejected() {
        let search = LocalSearchAuthority::new(ClientResourcePolicy::default_bounded());
        assert_eq!(
            search
                .query(
                    SearchNamespace::ProviderHistory,
                    SearchCursor::origin(SearchNamespace::ProtectedCommunication),
                )
                .expect_err("mismatch")
                .code(),
            "search_namespace_mismatch"
        );
    }
}

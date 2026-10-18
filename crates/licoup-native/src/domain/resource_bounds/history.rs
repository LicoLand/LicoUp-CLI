//! Streaming history page selection: identity-slot HashMap and a stable order
//! vector. Pages are keyset slices; total stays unknown until a scan completes.

use super::policy::ClientResourcePolicy;
use std::collections::HashMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentitySlot {
    pub identity: String,
    pub rank: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryPage {
    pub identities: Vec<String>,
    pub next_cursor: Option<u64>,
    pub total: Option<u32>,
}

/// Selects a bounded page without materializing a second collection.
pub struct HistoryPageSelector {
    policy: ClientResourcePolicy,
    slots: HashMap<String, u32>,
    order: Vec<String>,
}

impl HistoryPageSelector {
    pub fn new(policy: ClientResourcePolicy) -> Self {
        Self {
            policy,
            slots: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub fn observe(&mut self, identity: impl Into<String>, rank: u64) {
        let identity = identity.into();
        if self.slots.contains_key(&identity) {
            return;
        }
        let index = self.order.len() as u32;
        self.slots.insert(identity.clone(), index);
        self.order.push(identity);
        let _ = rank;
    }

    pub fn page(&self, load_index: u32, cursor: u64) -> HistoryPage {
        let limit = self.policy.history_page_size(load_index) as usize;
        let start = cursor as usize;
        if start >= self.order.len() {
            return HistoryPage {
                identities: Vec::new(),
                next_cursor: None,
                total: None,
            };
        }
        let end = (start + limit).min(self.order.len());
        HistoryPage {
            identities: self.order[start..end].to_vec(),
            next_cursor: (end < self.order.len()).then_some(end as u64),
            total: None,
        }
    }

    pub fn unique_slots(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::resource_bounds::policy::ClientResourcePolicy;

    #[test]
    fn small_page_does_not_require_the_full_identity_set_to_sort() {
        let mut selector = HistoryPageSelector::new(ClientResourcePolicy::default_bounded());
        for index in 0..400 {
            selector.observe(format!("id-{index:04}"), index as u64);
        }
        let first = selector.page(0, 0);
        assert_eq!(first.identities.len(), 50);
        assert_eq!(first.identities[0], "id-0000");
        assert_eq!(first.total, None);
        let second = selector.page(1, first.next_cursor.expect("next"));
        assert_eq!(second.identities.len(), 50);
        let later = selector.page(2, second.next_cursor.expect("next"));
        assert_eq!(later.identities.len(), 100);
        assert_eq!(selector.unique_slots(), 400);
    }

    #[test]
    fn duplicate_identities_occupy_one_slot() {
        let mut selector = HistoryPageSelector::new(ClientResourcePolicy::default_bounded());
        selector.observe("same", 1);
        selector.observe("same", 9);
        selector.observe("other", 2);
        let page = selector.page(0, 0);
        assert_eq!(page.identities, vec!["same", "other"]);
    }
}

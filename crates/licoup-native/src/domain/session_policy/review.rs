//! Review descriptors bind subject, destination, operation family, content
//! range, and consequence. Flutter may submit only the matching review id.

use super::policy::OperationFamily;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ReviewId(u64);

impl ReviewId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewDescriptor {
    pub subject: String,
    pub destination: String,
    pub operation_family: OperationFamily,
    pub content_range: ContentRange,
    pub consequence: ReviewConsequence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentRange {
    ExactIdentity,
    SelectedFile,
    ConversationMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewConsequence {
    UpdateContactIdentity,
    UnlockSession,
    ChangeProviderTrust,
    UseProtectedKey,
}

impl ReviewDescriptor {
    pub fn contact_identity(subject: impl Into<String>, destination: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            destination: destination.into(),
            operation_family: OperationFamily::ContactIdentityChange,
            content_range: ContentRange::ExactIdentity,
            consequence: ReviewConsequence::UpdateContactIdentity,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewDecision {
    pub review_id: ReviewId,
    pub approved: bool,
}

impl ReviewDecision {
    pub const fn approve(review_id: ReviewId) -> Self {
        Self {
            review_id,
            approved: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewError {
    UnknownReview,
    AlreadyDecided,
    SessionLocked,
}

impl ReviewError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnknownReview => "review_unknown",
            Self::AlreadyDecided => "review_already_decided",
            Self::SessionLocked => "review_session_locked",
        }
    }
}

pub(super) struct ReviewTable {
    next_id: u64,
    open: BTreeMap<ReviewId, ReviewDescriptor>,
}

impl ReviewTable {
    pub(super) fn new() -> Self {
        Self {
            next_id: 1,
            open: BTreeMap::new(),
        }
    }

    pub(super) fn open(&mut self, descriptor: ReviewDescriptor) -> Result<ReviewId, ReviewError> {
        let id = ReviewId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.open.insert(id, descriptor);
        Ok(id)
    }

    pub(super) fn decide(
        &mut self,
        decision: ReviewDecision,
    ) -> Result<ReviewDescriptor, ReviewError> {
        self.open
            .remove(&decision.review_id)
            .ok_or(ReviewError::UnknownReview)
    }

    pub(super) fn invalidate_all(&mut self) {
        self.open.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flutter_cannot_approve_a_different_review_id() {
        let mut table = ReviewTable::new();
        let id = table
            .open(ReviewDescriptor::contact_identity("c1", "d1"))
            .expect("open");
        let mismatch = ReviewDecision {
            review_id: ReviewId(id.get() + 9),
            approved: true,
        };
        assert_eq!(
            table.decide(mismatch).expect_err("mismatch").code(),
            "review_unknown"
        );
        table
            .decide(ReviewDecision::approve(id))
            .expect("matching id");
        assert_eq!(
            table
                .decide(ReviewDecision::approve(id))
                .expect_err("second")
                .code(),
            "review_unknown"
        );
    }
}

//! Confirmation and authentication are computed together from the trusted
//! session and operation family. Flutter cannot override these fields.

use super::session::{ClientSession, SessionError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationFamily {
    Add,
    Chat,
    Send,
    SelectedFile,
    ContactIdentityChange,
    AppUnlock,
    ProviderTrust,
    KeyCustody,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmationPolicy {
    DirectInteraction,
    Review,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthenticationRequirement {
    DeviceUnlocked,
    FreshUserPresence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationPolicy {
    pub family: OperationFamily,
    pub confirmation: ConfirmationPolicy,
    pub authentication: AuthenticationRequirement,
}

impl OperationPolicy {
    pub fn evaluate(session: ClientSession, family: OperationFamily) -> Result<Self, SessionError> {
        if session.is_locked() && family != OperationFamily::AppUnlock {
            return Err(SessionError::Locked);
        }
        let (confirmation, authentication) = match family {
            OperationFamily::Add
            | OperationFamily::Chat
            | OperationFamily::Send
            | OperationFamily::SelectedFile => (
                ConfirmationPolicy::DirectInteraction,
                AuthenticationRequirement::DeviceUnlocked,
            ),
            OperationFamily::ContactIdentityChange => (
                ConfirmationPolicy::Review,
                AuthenticationRequirement::DeviceUnlocked,
            ),
            OperationFamily::AppUnlock
            | OperationFamily::ProviderTrust
            | OperationFamily::KeyCustody => (
                ConfirmationPolicy::Review,
                AuthenticationRequirement::FreshUserPresence,
            ),
        };
        Ok(Self {
            family,
            confirmation,
            authentication,
        })
    }

    pub fn as_endpoint_policy(self) -> licoup_endpoint_core::OperationPolicy {
        licoup_endpoint_core::OperationPolicy {
            confirmation: match self.confirmation {
                ConfirmationPolicy::DirectInteraction => {
                    licoup_endpoint_core::ConfirmationRequirement::None
                }
                ConfirmationPolicy::Review => {
                    if self.authentication == AuthenticationRequirement::FreshUserPresence {
                        licoup_endpoint_core::ConfirmationRequirement::FreshUserPresence
                    } else {
                        licoup_endpoint_core::ConfirmationRequirement::Review
                    }
                }
            },
            authentication: match self.authentication {
                AuthenticationRequirement::DeviceUnlocked => {
                    licoup_endpoint_core::AuthenticationRequirement::None
                }
                AuthenticationRequirement::FreshUserPresence => {
                    licoup_endpoint_core::AuthenticationRequirement::NativeUserPresence
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::session_policy::session::{ClientSession, LockReason, SessionCommand};

    #[test]
    fn ordinary_add_chat_send_file_stay_direct() {
        let session = ClientSession::default_unlocked();
        for family in [
            OperationFamily::Add,
            OperationFamily::Chat,
            OperationFamily::Send,
            OperationFamily::SelectedFile,
        ] {
            let policy = OperationPolicy::evaluate(session, family).expect("policy");
            assert_eq!(policy.confirmation, ConfirmationPolicy::DirectInteraction);
            assert_eq!(
                policy.authentication,
                AuthenticationRequirement::DeviceUnlocked
            );
        }
    }

    #[test]
    fn contact_identity_is_review_without_fresh_presence() {
        let policy = OperationPolicy::evaluate(
            ClientSession::default_unlocked(),
            OperationFamily::ContactIdentityChange,
        )
        .expect("policy");
        assert_eq!(policy.confirmation, ConfirmationPolicy::Review);
        assert_eq!(
            policy.authentication,
            AuthenticationRequirement::DeviceUnlocked
        );
    }

    #[test]
    fn locked_session_rejects_ordinary_work_but_allows_unlock_policy() {
        let locked = ClientSession::default_unlocked()
            .reduce(SessionCommand::Lock(LockReason::Explicit))
            .expect("lock");
        assert!(OperationPolicy::evaluate(locked, OperationFamily::Send).is_err());
        let unlock = OperationPolicy::evaluate(locked, OperationFamily::AppUnlock).expect("unlock");
        assert_eq!(
            unlock.authentication,
            AuthenticationRequirement::FreshUserPresence
        );
    }
}

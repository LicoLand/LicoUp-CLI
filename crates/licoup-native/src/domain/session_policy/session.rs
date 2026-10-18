//! Closed ClientSession enum with a table-driven reducer.
//! App lock defaults off. An unlocked device session supports ordinary work.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientSession {
    Unlocked {
        security_generation: u64,
    },
    Locked {
        security_generation: u64,
        reason: LockReason,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockReason {
    Explicit,
    ProcessExit,
    OsLock,
    SecurityGenerationChanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionCommand {
    Unlock,
    Lock(LockReason),
    SecurityGenerationBump,
    ProcessExit,
    OsLock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionError {
    Locked,
    InvalidTransition,
}

impl SessionError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Locked => "session_locked",
            Self::InvalidTransition => "session_invalid_transition",
        }
    }
}

impl ClientSession {
    pub const fn default_unlocked() -> Self {
        Self::Unlocked {
            security_generation: 1,
        }
    }

    pub const fn is_locked(self) -> bool {
        matches!(self, Self::Locked { .. })
    }

    pub const fn security_generation(self) -> u64 {
        match self {
            Self::Unlocked {
                security_generation,
            }
            | Self::Locked {
                security_generation,
                ..
            } => security_generation,
        }
    }

    pub fn reduce(self, command: SessionCommand) -> Result<Self, SessionError> {
        match (self, command) {
            (
                Self::Locked {
                    security_generation,
                    ..
                },
                SessionCommand::Unlock,
            ) => Ok(Self::Unlocked {
                security_generation,
            }),
            (
                Self::Unlocked {
                    security_generation,
                },
                SessionCommand::Lock(reason),
            ) => Ok(Self::Locked {
                security_generation,
                reason,
            }),
            (
                Self::Locked {
                    security_generation,
                    ..
                },
                SessionCommand::Lock(reason),
            ) => Ok(Self::Locked {
                security_generation,
                reason,
            }),
            (session, SessionCommand::ProcessExit) => Ok(Self::Locked {
                security_generation: session.security_generation(),
                reason: LockReason::ProcessExit,
            }),
            (session, SessionCommand::OsLock) => Ok(Self::Locked {
                security_generation: session.security_generation(),
                reason: LockReason::OsLock,
            }),
            (session, SessionCommand::SecurityGenerationBump) => Ok(Self::Locked {
                security_generation: session.security_generation().saturating_add(1),
                reason: LockReason::SecurityGenerationChanged,
            }),
            (Self::Unlocked { .. }, SessionCommand::Unlock) => Ok(self),
        }
    }

    pub const fn invalidates_native_context(self, previous: Self) -> bool {
        match (previous, self) {
            (Self::Unlocked { .. }, Self::Locked { .. }) => true,
            (
                Self::Unlocked {
                    security_generation: before,
                }
                | Self::Locked {
                    security_generation: before,
                    ..
                },
                Self::Unlocked {
                    security_generation: after,
                }
                | Self::Locked {
                    security_generation: after,
                    ..
                },
            ) => after != before,
        }
    }
}

impl From<ClientSession> for licoup_endpoint_core::ClientSession {
    fn from(session: ClientSession) -> Self {
        match session {
            ClientSession::Unlocked { .. } => Self::device_unlocked(),
            ClientSession::Locked {
                reason: LockReason::ProcessExit,
                ..
            } => Self::terminated(),
            ClientSession::Locked { .. } => Self::app_locked(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_session_is_unlocked_with_app_lock_off() {
        let session = ClientSession::default_unlocked();
        assert!(!session.is_locked());
        assert_eq!(session.security_generation(), 1);
    }

    #[test]
    fn explicit_lock_and_os_lock_and_exit_share_the_locked_state() {
        let unlocked = ClientSession::default_unlocked();
        let explicit = unlocked
            .reduce(SessionCommand::Lock(LockReason::Explicit))
            .expect("lock");
        let os = unlocked.reduce(SessionCommand::OsLock).expect("os");
        let exit = unlocked.reduce(SessionCommand::ProcessExit).expect("exit");
        assert!(explicit.is_locked());
        assert!(os.is_locked());
        assert!(exit.is_locked());
        assert!(explicit.invalidates_native_context(unlocked));
    }

    #[test]
    fn security_generation_bump_locks_and_changes_generation() {
        let unlocked = ClientSession::default_unlocked();
        let next = unlocked
            .reduce(SessionCommand::SecurityGenerationBump)
            .expect("bump");
        assert!(next.is_locked());
        assert_eq!(next.security_generation(), 2);
        assert!(next.invalidates_native_context(unlocked));
    }
}

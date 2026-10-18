//! Test-visible interrupt and clock hooks. These never store user content.

use std::cell::Cell;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContinuityInterrupt {
    BeforeFirstWrite,
    AfterStateWrite,
    BeforeCommit,
    AfterCommit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContinuityEffectStatus {
    NotExecuted,
    Executed,
    Unknown,
}

impl ContinuityEffectStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotExecuted => "not_executed",
            Self::Executed => "executed",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "not_executed" => Some(Self::NotExecuted),
            "executed" => Some(Self::Executed),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

thread_local! {
    static INTERRUPT: Cell<Option<ContinuityInterrupt>> = const { Cell::new(None) };
    static CLOCK_MS: Cell<Option<i64>> = const { Cell::new(None) };
}

#[doc(hidden)]
pub fn set_continuity_interrupt(point: Option<ContinuityInterrupt>) {
    INTERRUPT.with(|cell| cell.set(point));
}

#[doc(hidden)]
pub fn set_continuity_clock(now_ms: Option<i64>) {
    CLOCK_MS.with(|cell| cell.set(now_ms));
}

pub(crate) fn take_interrupt(expected: ContinuityInterrupt) -> bool {
    INTERRUPT.with(|cell| {
        if cell.get() == Some(expected) {
            cell.set(None);
            true
        } else {
            false
        }
    })
}

pub fn continuity_now_ms() -> i64 {
    CLOCK_MS.with(|cell| {
        cell.get().unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0)
        })
    })
}

//! Generation-index handle arena. The slot implementation lives in
//! `licoup-platform-bridges`; this adapter tags each arena with a handle kind
//! so stale and capacity failures stay typed.

use super::abi::RuntimeError;
use licoup_platform_bridges::{ArenaError, HandleArena as BridgedArena};

pub use licoup_platform_bridges::Handle;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HandleKind {
    Runtime,
    Future,
    Subscription,
    SharedBuffer,
}

/// Bounded generational arena. A retired slot (generation wrap) is never reused.
pub struct HandleArena<T> {
    inner: BridgedArena<T>,
    kind: HandleKind,
}

impl<T> HandleArena<T> {
    pub fn with_capacity(kind: HandleKind, capacity: u32) -> Self {
        Self {
            inner: BridgedArena::bounded(capacity as usize),
            kind,
        }
    }

    pub fn capacity(&self) -> u32 {
        self.inner.capacity() as u32
    }

    pub fn live_count(&self) -> u32 {
        self.inner.len() as u32
    }

    pub fn allocate(&mut self, value: T) -> Result<Handle, RuntimeError> {
        self.inner
            .insert(value)
            .map_err(|error| self.map_error(error))
    }

    pub fn get(&self, handle: Handle) -> Result<&T, RuntimeError> {
        self.inner
            .get(handle)
            .ok_or(RuntimeError::StaleHandle { kind: self.kind })
    }

    pub fn get_mut(&mut self, handle: Handle) -> Result<&mut T, RuntimeError> {
        let kind = self.kind;
        self.inner
            .get_mut(handle)
            .ok_or(RuntimeError::StaleHandle { kind })
    }

    pub fn free(&mut self, handle: Handle) -> Result<T, RuntimeError> {
        self.inner
            .free(handle)
            .map_err(|error| self.map_error(error))
    }

    fn map_error(&self, error: ArenaError) -> RuntimeError {
        match error {
            ArenaError::CapacityExceeded => RuntimeError::CapacityExceeded {
                kind: self.kind,
                capacity: self.capacity(),
            },
            ArenaError::StaleHandle => RuntimeError::StaleHandle { kind: self.kind },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_get_free_rejects_stale_handle() {
        let mut arena = HandleArena::with_capacity(HandleKind::Future, 4);
        let handle = arena.allocate(7_u32).expect("allocate");
        assert_eq!(*arena.get(handle).expect("get"), 7);
        assert_eq!(arena.free(handle).expect("free"), 7);
        assert!(matches!(
            arena.get(handle),
            Err(RuntimeError::StaleHandle {
                kind: HandleKind::Future
            })
        ));
        assert!(matches!(
            arena.free(handle),
            Err(RuntimeError::StaleHandle {
                kind: HandleKind::Future
            })
        ));
    }

    #[test]
    fn reused_slot_issues_a_new_generation() {
        let mut arena = HandleArena::with_capacity(HandleKind::Future, 2);
        let first = arena.allocate(1_u32).expect("first");
        arena.free(first).expect("free");
        let second = arena.allocate(2_u32).expect("second");
        assert_eq!(first.index(), second.index());
        assert_ne!(first.generation(), second.generation());
        assert!(arena.get(first).is_err());
        assert_eq!(*arena.get(second).expect("live"), 2);
    }

    #[test]
    fn capacity_is_a_typed_failure() {
        let mut arena = HandleArena::with_capacity(HandleKind::SharedBuffer, 1);
        let _live = arena.allocate(1_u32).expect("live");
        assert!(matches!(
            arena.allocate(2_u32),
            Err(RuntimeError::CapacityExceeded {
                kind: HandleKind::SharedBuffer,
                capacity: 1
            })
        ));
        assert_eq!(arena.live_count(), 1);
    }
}

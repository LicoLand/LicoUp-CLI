//! Monotonic stream cursors, latest-state merge, and dual-bound backpressure.

use super::abi::{RuntimeError, StreamReplayClass};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct StreamCursor {
    position: u64,
}

impl StreamCursor {
    pub const fn origin() -> Self {
        Self { position: 0 }
    }

    pub const fn position(self) -> u64 {
        self.position
    }

    pub const fn next(self) -> Self {
        Self {
            position: self.position.saturating_add(1),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamItem {
    pub cursor: StreamCursor,
    pub revision: u64,
    pub bytes: Vec<u8>,
}

/// Latest-state projections keep only the newest revision while the cursor
/// still advances so a subscriber can detect missed coalescing.
#[derive(Clone, Debug, Default)]
pub struct LatestStateMerge {
    cursor: StreamCursor,
    revision: u64,
    payload: Option<Vec<u8>>,
}

impl LatestStateMerge {
    pub fn apply(&mut self, payload: Vec<u8>) -> StreamCursor {
        self.revision = self.revision.saturating_add(1);
        self.cursor = self.cursor.next();
        self.payload = Some(payload);
        self.cursor
    }

    pub fn snapshot(&self) -> Option<(StreamCursor, u64, &[u8])> {
        self.payload
            .as_deref()
            .map(|bytes| (self.cursor, self.revision, bytes))
    }
}

/// Dual-bounded in-memory queue. Overflow is reported so the caller can spool
/// complete Agent output; this queue never truncates silently.
pub struct StreamQueue {
    replay: StreamReplayClass,
    items: VecDeque<StreamItem>,
    next_cursor: StreamCursor,
    bytes: usize,
    max_events: usize,
    max_bytes: usize,
}

impl StreamQueue {
    pub fn new(replay: StreamReplayClass, max_events: usize, max_bytes: usize) -> Self {
        Self {
            replay,
            items: VecDeque::new(),
            next_cursor: StreamCursor::origin(),
            bytes: 0,
            max_events: max_events.max(1),
            max_bytes: max_bytes.max(1),
        }
    }

    pub fn replay_class(&self) -> StreamReplayClass {
        self.replay
    }

    pub fn next_cursor(&self) -> StreamCursor {
        self.next_cursor
    }

    pub fn push(&mut self, payload: Vec<u8>) -> Result<StreamCursor, RuntimeError> {
        if self.items.len() >= self.max_events
            || self.bytes.saturating_add(payload.len()) > self.max_bytes
        {
            return Err(RuntimeError::CapacityExceeded {
                kind: super::arena::HandleKind::Subscription,
                capacity: self.max_events as u32,
            });
        }
        let cursor = self.next_cursor.next();
        self.next_cursor = cursor;
        self.bytes = self.bytes.saturating_add(payload.len());
        self.items.push_back(StreamItem {
            cursor,
            revision: cursor.position(),
            bytes: payload,
        });
        Ok(cursor)
    }

    pub fn drain_from(
        &mut self,
        cursor: StreamCursor,
        limit: usize,
    ) -> Result<Vec<StreamItem>, RuntimeError> {
        if cursor.position() > self.next_cursor.position() {
            return Err(RuntimeError::CursorInvalid {
                earliest: self.earliest_position(),
            });
        }
        let earliest = self.earliest_position();
        let min_valid = earliest.saturating_sub(1);
        if cursor.position() < min_valid {
            return Err(RuntimeError::CursorInvalid { earliest });
        }
        let mut drained = Vec::new();
        while drained.len() < limit {
            let Some(front) = self.items.front() else {
                break;
            };
            if front.cursor.position() <= cursor.position() {
                if let Some(item) = self.items.pop_front() {
                    self.bytes = self.bytes.saturating_sub(item.bytes.len());
                }
                continue;
            }
            if let Some(item) = self.items.pop_front() {
                self.bytes = self.bytes.saturating_sub(item.bytes.len());
                drained.push(item);
            }
        }
        Ok(drained)
    }

    pub fn earliest_position(&self) -> u64 {
        self.items
            .front()
            .map(|item| item.cursor.position())
            .unwrap_or(self.next_cursor.position())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_state_coalesces_payload_and_advances_cursor() {
        let mut merge = LatestStateMerge::default();
        let first = merge.apply(b"rev-1".to_vec());
        let second = merge.apply(b"rev-2".to_vec());
        assert!(second > first);
        let (cursor, revision, payload) = merge.snapshot().expect("payload");
        assert_eq!(cursor, second);
        assert_eq!(revision, 2);
        assert_eq!(payload, b"rev-2");
    }

    #[test]
    fn ordered_queue_backpressures_instead_of_truncating() {
        let mut queue = StreamQueue::new(StreamReplayClass::Ordered, 2, 16);
        queue.push(b"one".to_vec()).expect("one");
        queue.push(b"two".to_vec()).expect("two");
        assert!(matches!(
            queue.push(b"three".to_vec()),
            Err(RuntimeError::CapacityExceeded { .. })
        ));
        let drained = queue.drain_from(StreamCursor::origin(), 8).expect("drain");
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].bytes, b"one");
        assert_eq!(drained[1].bytes, b"two");
    }

    #[test]
    fn stale_cursor_returns_earliest_available_position() {
        let mut queue = StreamQueue::new(StreamReplayClass::Ordered, 2, 64);
        queue.push(vec![1]).expect("first");
        queue.push(vec![2]).expect("second");
        let drained = queue
            .drain_from(StreamCursor::origin(), 2)
            .expect("consume");
        assert_eq!(drained.len(), 2);
        let error = queue
            .drain_from(StreamCursor::origin(), 1)
            .expect_err("origin behind dropped prefix");
        assert!(matches!(error, RuntimeError::CursorInvalid { earliest } if earliest >= 2));
    }
}

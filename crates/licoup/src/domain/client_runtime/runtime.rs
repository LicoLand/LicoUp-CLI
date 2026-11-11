//! Linearized runtime state machine: futures, subscriptions, shared buffers,
//! and wake-only callbacks. Teardown, cancel, and late callbacks share one
//! transition table.

use super::abi::{RuntimeCommand, RuntimeError, StreamReplayClass};
use super::agent_ipc::AgentPrivateIpc;
use super::arena::{Handle, HandleArena, HandleKind};
use super::spool::OutputSpool;
use super::stream::{StreamCursor, StreamItem, StreamQueue};
use std::sync::Arc;

pub type WakeCallback = Arc<dyn Fn() + Send + Sync>;

const DEFAULT_HANDLE_CAPACITY: u32 = 256;
const DEFAULT_STREAM_EVENTS: usize = 64;
const DEFAULT_STREAM_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FutureState {
    Pending,
    Ready,
    Cancelled,
    Freed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionState {
    Open,
    Cancelled,
    Freed,
}

struct FutureSlot {
    state: FutureState,
    result: Option<Vec<u8>>,
    wake: Option<WakeCallback>,
}

struct SubscriptionSlot {
    state: SubscriptionState,
    queue: StreamQueue,
    spool: OutputSpool,
    wake: Option<WakeCallback>,
}

struct SharedBufferSlot {
    bytes: Vec<u8>,
}

/// In-process runtime. The ABI is create/destroy plus handle operations.
pub struct ClientRuntime {
    runtime: HandleArena<()>,
    futures: HandleArena<FutureSlot>,
    subscriptions: HandleArena<SubscriptionSlot>,
    buffers: HandleArena<SharedBufferSlot>,
    agent_ipc: AgentPrivateIpc,
    live: Option<Handle>,
}

impl ClientRuntime {
    pub fn new() -> Self {
        Self {
            runtime: HandleArena::with_capacity(HandleKind::Runtime, 1),
            futures: HandleArena::with_capacity(HandleKind::Future, DEFAULT_HANDLE_CAPACITY),
            subscriptions: HandleArena::with_capacity(
                HandleKind::Subscription,
                DEFAULT_HANDLE_CAPACITY,
            ),
            buffers: HandleArena::with_capacity(HandleKind::SharedBuffer, DEFAULT_HANDLE_CAPACITY),
            agent_ipc: AgentPrivateIpc::bounded(32),
            live: None,
        }
    }

    pub fn create(&mut self) -> Result<Handle, RuntimeError> {
        if self.live.is_some() {
            return Err(RuntimeError::InvalidState);
        }
        let handle = self.runtime.allocate(())?;
        self.live = Some(handle);
        Ok(handle)
    }

    pub fn destroy(&mut self, handle: Handle) -> Result<(), RuntimeError> {
        self.require_live(handle)?;
        self.drop_outstanding();
        self.runtime.free(handle)?;
        self.live = None;
        self.agent_ipc.close();
        Ok(())
    }

    pub fn agent_ipc(&mut self) -> &mut AgentPrivateIpc {
        &mut self.agent_ipc
    }

    pub fn spawn_future(&mut self, wake: WakeCallback) -> Result<Handle, RuntimeError> {
        self.require_any_live()?;
        self.futures.allocate(FutureSlot {
            state: FutureState::Pending,
            result: None,
            wake: Some(wake),
        })
    }

    pub fn complete_future(&mut self, handle: Handle, result: Vec<u8>) -> Result<(), RuntimeError> {
        let slot = self.futures.get_mut(handle)?;
        match slot.state {
            FutureState::Pending => {
                slot.state = FutureState::Ready;
                slot.result = Some(result);
                if let Some(wake) = slot.wake.take() {
                    wake();
                }
                Ok(())
            }
            FutureState::Cancelled | FutureState::Freed => Ok(()),
            FutureState::Ready => Err(RuntimeError::AlreadyCompleted),
        }
    }

    pub fn poll_future(&mut self, handle: Handle) -> Result<Option<Vec<u8>>, RuntimeError> {
        let slot = self.futures.get_mut(handle)?;
        match slot.state {
            FutureState::Pending => Ok(None),
            FutureState::Ready => Ok(slot.result.clone()),
            FutureState::Cancelled => Err(RuntimeError::Cancelled),
            FutureState::Freed => Err(RuntimeError::StaleHandle {
                kind: HandleKind::Future,
            }),
        }
    }

    pub fn cancel_future(&mut self, handle: Handle) -> Result<(), RuntimeError> {
        let slot = self.futures.get_mut(handle)?;
        match slot.state {
            FutureState::Pending | FutureState::Ready => {
                slot.state = FutureState::Cancelled;
                slot.result = None;
                slot.wake = None;
                Ok(())
            }
            FutureState::Cancelled => Ok(()),
            FutureState::Freed => Err(RuntimeError::StaleHandle {
                kind: HandleKind::Future,
            }),
        }
    }

    pub fn free_future(&mut self, handle: Handle) -> Result<(), RuntimeError> {
        let mut slot = self.futures.free(handle)?;
        slot.state = FutureState::Freed;
        slot.wake = None;
        slot.result = None;
        Ok(())
    }

    pub fn subscribe(
        &mut self,
        replay: StreamReplayClass,
        wake: WakeCallback,
    ) -> Result<Handle, RuntimeError> {
        self.require_any_live()?;
        self.subscriptions.allocate(SubscriptionSlot {
            state: SubscriptionState::Open,
            queue: StreamQueue::new(replay, DEFAULT_STREAM_EVENTS, DEFAULT_STREAM_BYTES),
            spool: OutputSpool::process_local(),
            wake: Some(wake),
        })
    }

    pub fn publish(
        &mut self,
        handle: Handle,
        payload: Vec<u8>,
    ) -> Result<StreamCursor, RuntimeError> {
        let slot = self.subscriptions.get_mut(handle)?;
        if slot.state != SubscriptionState::Open {
            return Err(RuntimeError::Cancelled);
        }
        match slot.queue.push(payload.clone()) {
            Ok(cursor) => {
                if let Some(wake) = &slot.wake {
                    wake();
                }
                Ok(cursor)
            }
            Err(RuntimeError::CapacityExceeded { .. }) => {
                slot.spool
                    .append(&payload)
                    .map_err(|_| RuntimeError::CapacityExceeded {
                        kind: HandleKind::Subscription,
                        capacity: DEFAULT_STREAM_EVENTS as u32,
                    })?;
                if let Some(wake) = &slot.wake {
                    wake();
                }
                Ok(slot.queue.next_cursor())
            }
            Err(error) => Err(error),
        }
    }

    pub fn drain_subscription(
        &mut self,
        handle: Handle,
        cursor: StreamCursor,
        limit: usize,
    ) -> Result<Vec<StreamItem>, RuntimeError> {
        let slot = self.subscriptions.get_mut(handle)?;
        if slot.state != SubscriptionState::Open {
            return Err(RuntimeError::Cancelled);
        }
        slot.queue.drain_from(cursor, limit)
    }

    pub fn cancel_subscription(&mut self, handle: Handle) -> Result<(), RuntimeError> {
        let slot = self.subscriptions.get_mut(handle)?;
        slot.state = SubscriptionState::Cancelled;
        slot.wake = None;
        Ok(())
    }

    pub fn free_subscription(&mut self, handle: Handle) -> Result<(), RuntimeError> {
        let mut slot = self.subscriptions.free(handle)?;
        slot.state = SubscriptionState::Freed;
        slot.wake = None;
        Ok(())
    }

    pub fn alloc_shared_buffer(&mut self, bytes: Vec<u8>) -> Result<Handle, RuntimeError> {
        self.require_any_live()?;
        self.buffers.allocate(SharedBufferSlot { bytes })
    }

    pub fn free_shared_buffer(&mut self, handle: Handle) -> Result<(), RuntimeError> {
        self.buffers.free(handle).map(|_| ())
    }

    pub fn shared_buffer_len(&self, handle: Handle) -> Result<usize, RuntimeError> {
        self.buffers.get(handle).map(|slot| slot.bytes.len())
    }

    pub fn dispatch(
        &mut self,
        command: RuntimeCommand,
        handle: Handle,
    ) -> Result<(), RuntimeError> {
        match command {
            RuntimeCommand::Destroy => self.destroy(handle),
            RuntimeCommand::FutureCancel => self.cancel_future(handle),
            RuntimeCommand::FutureFree => self.free_future(handle),
            RuntimeCommand::SubscriptionCancel => self.cancel_subscription(handle),
            RuntimeCommand::SubscriptionFree => self.free_subscription(handle),
            RuntimeCommand::SharedBufferFree => self.free_shared_buffer(handle),
            RuntimeCommand::Create
            | RuntimeCommand::FuturePoll
            | RuntimeCommand::FutureComplete
            | RuntimeCommand::SubscriptionDrain => Err(RuntimeError::InvalidState),
        }
    }

    fn require_live(&self, handle: Handle) -> Result<(), RuntimeError> {
        if self.live != Some(handle) {
            return Err(RuntimeError::StaleHandle {
                kind: HandleKind::Runtime,
            });
        }
        self.runtime.get(handle).map(|_| ())
    }

    fn require_any_live(&self) -> Result<(), RuntimeError> {
        match self.live {
            Some(handle) => self.require_live(handle),
            None => Err(RuntimeError::InvalidState),
        }
    }

    fn drop_outstanding(&mut self) {
        // Handles become unusable once the runtime is destroyed; arenas drop
        // with the struct. Wake callbacks are cleared by dropping slots.
    }
}

impl Default for ClientRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn wake_counter() -> (WakeCallback, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&count);
        let wake: WakeCallback = Arc::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        (wake, count)
    }

    #[test]
    fn wake_only_callback_does_not_carry_payload() {
        let mut runtime = ClientRuntime::new();
        let _root = runtime.create().expect("create");
        let (wake, count) = wake_counter();
        let future = runtime.spawn_future(wake).expect("spawn");
        runtime
            .complete_future(future, b"ready".to_vec())
            .expect("complete");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            runtime.poll_future(future).expect("poll").as_deref(),
            Some(b"ready".as_ref())
        );
    }

    #[test]
    fn late_complete_after_cancel_is_ignored() {
        let mut runtime = ClientRuntime::new();
        let _root = runtime.create().expect("create");
        let (wake, count) = wake_counter();
        let future = runtime.spawn_future(wake).expect("spawn");
        runtime.cancel_future(future).expect("cancel");
        runtime
            .complete_future(future, b"too-late".to_vec())
            .expect("late complete is linearized");
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(matches!(
            runtime.poll_future(future),
            Err(RuntimeError::Cancelled)
        ));
    }

    #[test]
    fn free_then_poll_is_stale() {
        let mut runtime = ClientRuntime::new();
        let _root = runtime.create().expect("create");
        let (wake, _) = wake_counter();
        let future = runtime.spawn_future(wake).expect("spawn");
        runtime.free_future(future).expect("free");
        assert!(matches!(
            runtime.poll_future(future),
            Err(RuntimeError::StaleHandle {
                kind: HandleKind::Future
            })
        ));
    }

    #[test]
    fn ordered_subscription_spools_complete_output_on_backpressure() {
        let mut runtime = ClientRuntime::new();
        let _root = runtime.create().expect("create");
        let (wake, count) = wake_counter();
        let sub = runtime
            .subscribe(StreamReplayClass::Ordered, wake)
            .expect("subscribe");
        for index in 0..(DEFAULT_STREAM_EVENTS + 2) {
            runtime
                .publish(sub, vec![index as u8])
                .expect("publish must keep complete output");
        }
        assert!(count.load(Ordering::SeqCst) >= DEFAULT_STREAM_EVENTS);
        let drained = runtime
            .drain_subscription(sub, StreamCursor::origin(), DEFAULT_STREAM_EVENTS)
            .expect("drain");
        assert_eq!(drained.len(), DEFAULT_STREAM_EVENTS);
    }

    #[test]
    fn destroy_closes_agent_ipc() {
        let mut runtime = ClientRuntime::new();
        let root = runtime.create().expect("create");
        runtime.destroy(root).expect("destroy");
        assert!(runtime.agent_ipc().is_closed());
    }

    #[test]
    fn shared_buffer_free_releases_the_handle() {
        let mut runtime = ClientRuntime::new();
        let _root = runtime.create().expect("create");
        let buffer = runtime
            .alloc_shared_buffer(b"payload".to_vec())
            .expect("alloc");
        assert_eq!(runtime.shared_buffer_len(buffer).expect("len"), 7);
        runtime.free_shared_buffer(buffer).expect("free");
        assert!(runtime.shared_buffer_len(buffer).is_err());
    }
}

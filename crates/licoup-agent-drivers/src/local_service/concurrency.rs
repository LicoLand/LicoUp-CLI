use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitFailure {
    Busy,
    Unavailable,
}

pub struct BoundedGate {
    capacity: usize,
    active: Mutex<usize>,
    ready: Condvar,
}

impl BoundedGate {
    pub const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            active: Mutex::new(0),
            ready: Condvar::new(),
        }
    }

    pub fn acquire(&self, timeout: Duration) -> Result<Permit<'_>, LimitFailure> {
        let deadline = Instant::now() + timeout;
        let mut active = self.active.lock().map_err(|_| LimitFailure::Unavailable)?;
        loop {
            if *active < self.capacity {
                *active += 1;
                return Ok(Permit { gate: self });
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(LimitFailure::Busy);
            }
            let remaining = deadline.saturating_duration_since(now);
            let (next, timeout_result) = self
                .ready
                .wait_timeout(active, remaining)
                .map_err(|_| LimitFailure::Unavailable)?;
            active = next;
            if timeout_result.timed_out() && *active >= self.capacity {
                return Err(LimitFailure::Busy);
            }
        }
    }
}

pub struct Permit<'a> {
    gate: &'a BoundedGate,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.gate.active.lock() {
            *active = active.saturating_sub(1);
            self.gate.ready.notify_one();
        }
    }
}

//! Optional, invocation-owned local inspection before public semantic projection.
//! Worker threads must explicitly carry an observer; no global dispatch is used.

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    cell::RefCell,
    marker::PhantomData,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawExecutionDirection {
    Sent,
    Received,
    Stderr,
}

impl RawExecutionDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Received => "received",
            Self::Stderr => "stderr",
        }
    }
}

type RawExecutionCallback = dyn Fn(&str, RawExecutionDirection, &str) -> Result<()> + Send + Sync;

#[derive(Clone)]
pub struct RawExecutionObserver {
    callback: Arc<Mutex<Option<Box<RawExecutionCallback>>>>,
    failed: Arc<AtomicBool>,
}

thread_local! {
    static CURRENT: RefCell<Option<RawExecutionObserver>> = const {RefCell::new(None)};
}

impl RawExecutionObserver {
    pub fn new(
        callback: impl Fn(&str, RawExecutionDirection, &str) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            callback: Arc::new(Mutex::new(Some(Box::new(callback)))),
            failed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn current() -> Option<Self> {
        CURRENT.with(|current| current.borrow().clone())
    }

    /// Persist the exact text supplied at the transport boundary. The callback
    /// serializes writes and completes before this call returns.
    pub fn record(&self, source: &str, direction: RawExecutionDirection, raw_text: &str) {
        match self.callback.lock() {
            Ok(callback) => {
                if let Some(callback) = callback.as_ref()
                    && callback(source, direction, raw_text).is_err()
                {
                    self.failed.store(true, Ordering::Release);
                }
            }
            Err(_) => self.failed.store(true, Ordering::Release),
        }
    }

    /// Invalid UTF-8 (including a transport chunk split inside a codepoint) is
    /// retained reversibly. The visible source kind explicitly names Base64.
    pub fn record_bytes(&self, source: &str, direction: RawExecutionDirection, bytes: &[u8]) {
        match std::str::from_utf8(bytes) {
            Ok(text) => self.record(source, direction, text),
            Err(_) => self.record(
                &format!("{source}.base64"),
                direction,
                &STANDARD.encode(bytes),
            ),
        }
    }

    pub fn had_failure(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    /// Drain any in-flight callback before terminal persistence. Late worker
    /// clones become inert and cannot retain a previous invocation's sink.
    pub fn close(&self) {
        self.callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
}

/// Thread-local binding only. This guard cannot move between worker threads.
pub struct RawExecutionScope {
    previous: Option<RawExecutionObserver>,
    _thread: PhantomData<Rc<()>>,
}

impl RawExecutionScope {
    pub fn enter(observer: Option<RawExecutionObserver>) -> Self {
        Self {
            previous: CURRENT.with(|current| current.replace(observer)),
            _thread: PhantomData,
        }
    }
}

impl Drop for RawExecutionScope {
    fn drop(&mut self) {
        CURRENT.with(|current| current.replace(self.previous.take()));
    }
}

/// One persistent transport's temporary receive owner. Hold the binding guard
/// for a single turn under that transport's existing execution serialization.
#[derive(Clone, Default)]
pub struct RawExecutionBinding {
    observer: Arc<Mutex<Option<RawExecutionObserver>>>,
}

impl std::fmt::Debug for RawExecutionBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RawExecutionBinding")
    }
}

#[derive(Debug)]
pub struct RawExecutionBindingGuard {
    binding: RawExecutionBinding,
}

impl RawExecutionBindingGuard {
    /// Transfer a spawn-time receive binding to the turn that actually acquired
    /// the persistent transport, without dropping the guard between owners.
    pub fn rebind_current(self) -> Self {
        *self
            .binding
            .observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = RawExecutionObserver::current();
        self
    }
}

impl RawExecutionBinding {
    pub fn bind(&self, observer: Option<RawExecutionObserver>) -> RawExecutionBindingGuard {
        *self
            .observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = observer;
        RawExecutionBindingGuard {
            binding: self.clone(),
        }
    }

    pub fn bind_current(&self) -> RawExecutionBindingGuard {
        self.bind(RawExecutionObserver::current())
    }

    pub fn record(&self, source: &str, direction: RawExecutionDirection, text: &str) {
        let observer = self
            .observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(observer) = observer {
            observer.record(source, direction, text);
        }
    }

    pub fn record_bytes(&self, source: &str, direction: RawExecutionDirection, bytes: &[u8]) {
        let observer = self
            .observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(observer) = observer {
            observer.record_bytes(source, direction, bytes);
        }
    }
}

impl Drop for RawExecutionBindingGuard {
    fn drop(&mut self) {
        self.binding
            .observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
}

/// Capture a successful physical read before BufReader can prefetch bytes for
/// another iteration. Ownership is the receive binding at read time.
pub struct RawExecutionReader<R> {
    reader: R,
    binding: RawExecutionBinding,
    source: &'static str,
    direction: RawExecutionDirection,
}

impl<R> RawExecutionReader<R> {
    pub fn new(
        reader: R,
        binding: RawExecutionBinding,
        source: &'static str,
        direction: RawExecutionDirection,
    ) -> Self {
        Self {
            reader,
            binding,
            source,
            direction,
        }
    }
}

impl<R: std::io::Read> std::io::Read for RawExecutionReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.reader.read(buffer)?;
        if count > 0 {
            self.binding
                .record_bytes(self.source, self.direction, &buffer[..count]);
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_execution_binding_releases_receive_ownership_and_preserves_invalid_bytes() {
        let records = Arc::new(Mutex::new(Vec::new()));
        let observer = |label: &'static str| {
            let records = Arc::clone(&records);
            RawExecutionObserver::new(move |source, _, text| {
                records
                    .lock()
                    .unwrap()
                    .push((label, source.to_owned(), text.to_owned()));
                Ok(())
            })
        };
        let binding = RawExecutionBinding::default();
        let first = observer("first");
        {
            let _guard = binding.bind(Some(first.clone()));
            binding.record_bytes(
                "synthetic",
                RawExecutionDirection::Received,
                b"\xff\x80\x00",
            );
        }
        binding.record(
            "synthetic",
            RawExecutionDirection::Received,
            "unowned idle frame",
        );
        let second = observer("second");
        {
            let _guard = binding.bind(Some(second.clone()));
            binding.record(
                "synthetic",
                RawExecutionDirection::Received,
                "second owned frame",
            );
        }
        first.close();
        second.close();
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].0, "first");
        assert_eq!(records[0].1, "synthetic.base64");
        assert_eq!(STANDARD.decode(&records[0].2).unwrap(), b"\xff\x80\x00");
        assert_eq!(records[1].0, "second");
    }

    #[test]
    fn raw_execution_capture_failure_is_sticky_without_returning_execution_error() {
        let observer =
            RawExecutionObserver::new(|_, _, _| Err(anyhow::anyhow!("synthetic capture fault")));
        observer.record(
            "synthetic",
            RawExecutionDirection::Sent,
            "sent despite capture failure",
        );
        observer.close();
        assert!(observer.had_failure());
    }

    #[test]
    fn raw_execution_scopes_restore_parent_and_require_explicit_worker_ownership() {
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&records);
        let first = RawExecutionObserver::new(move |source, direction, text| {
            sink.lock()
                .unwrap()
                .push((source.to_owned(), direction, text.to_owned()));
            Ok(())
        });
        let _scope = RawExecutionScope::enter(Some(first.clone()));
        std::thread::spawn(|| assert!(RawExecutionObserver::current().is_none()))
            .join()
            .unwrap();
        {
            let _nested = RawExecutionScope::enter(None);
            assert!(RawExecutionObserver::current().is_none());
        }
        RawExecutionObserver::current().unwrap().record(
            "synthetic",
            RawExecutionDirection::Sent,
            " [ 1, {\"unknown\":true} ] \n",
        );
        let worker = first.clone();
        std::thread::spawn(move || {
            worker.record("synthetic", RawExecutionDirection::Received, "raw reply")
        })
        .join()
        .unwrap();
        first.close();
        first.record(
            "synthetic",
            RawExecutionDirection::Received,
            "late previous turn",
        );
        assert_eq!(records.lock().unwrap().len(), 2);
        assert_eq!(
            records.lock().unwrap()[0].2,
            " [ 1, {\"unknown\":true} ] \n"
        );
    }
}

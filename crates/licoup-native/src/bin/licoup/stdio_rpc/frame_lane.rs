//! Producer-side frame classification and the bounded bulk write budget.
//!
//! This process writes every frame on one stdout pipe. A control reply
//! (cancel/steer/detach/stop/force) is therefore only reachable after the bytes
//! written before it, and the client cannot pause that pipe without also
//! stopping short of the control frame behind the bulk. The producer's half of
//! that contract is here:
//!
//! * every frame names its lane at the write site, so a frame cannot be written
//!   without a classification;
//! * a frame written by a bulk-producing worker thread waits for a bounded
//!   budget before it takes the writer. That wait is the backpressure on the
//!   bulk source, and it keeps the queue of bulk frames in front of a control
//!   frame bounded by [STDIO_RPC_BULK_WRITE_MAX_FRAMES] frames and
//!   [STDIO_RPC_BULK_WRITE_HIGH_WATERMARK_BYTES] bytes;
//! * control frames and the request loop's own inline replies never wait for
//!   that budget. The loop is serial: its own frame cannot queue behind another
//!   frame of its lane, and making it wait would stop it reading the very
//!   control request it should answer.
//!
//! Frames stay whole lines: one frame is serialized, written and flushed under
//! the writer lock, because the client's line framer splits on newlines and
//! interleaved frames would corrupt the stream for every consumer.

use super::*;
use std::sync::Condvar;

/// Where one outbound frame sits relative to the shared stdout writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StdioRpcFrameLane {
    /// Reply to an in-flight control request (cancel/steer/detach/stop/force).
    /// Never waits for the bulk write budget: this is the frame the bulk budget
    /// exists to keep reachable.
    Control,
    /// A reply the request loop writes inline before it reads its next request.
    /// Never waits for the bulk write budget either: the loop is serial, so its
    /// frame cannot queue behind another frame of its own lane, and waiting
    /// would stop it from reading the next request.
    Ordered,
    /// Stream output written by a worker thread (turn events, terminals,
    /// admitted blocking-command results). Waits for the bounded bulk write
    /// budget, which is the real backpressure toward the bulk source.
    Stream,
}

/// Most bulk frames that may be inside the writer at once.
///
/// One is being written and at most one more is waiting for the writer, so a
/// control frame can never queue behind a deep queue of bulk frames.
pub(crate) const STDIO_RPC_BULK_WRITE_MAX_FRAMES: usize = 2;

/// Bulk frames admitted again only down to this count after saturation.
pub(crate) const STDIO_RPC_BULK_WRITE_LOW_FRAMES: usize = 1;

/// Bulk bytes that may be admitted-but-unwritten at once: the same bound as one
/// protocol-maximum response, so a single maximum frame is admitted alone.
pub(crate) const STDIO_RPC_BULK_WRITE_HIGH_WATERMARK_BYTES: usize = STDIO_RPC_MAX_RESPONSE_BYTES;

/// Bulk bytes admitted again only down to this watermark after saturation.
pub(crate) const STDIO_RPC_BULK_WRITE_LOW_WATERMARK_BYTES: usize = STDIO_RPC_MAX_RESPONSE_BYTES / 4;

#[derive(Default)]
struct BulkWriteState {
    admitted_frames: usize,
    admitted_bytes: usize,
    saturated: bool,
}

/// Whether one frame of `frame_bytes` fits the budget right now.
///
/// Nothing is admitted above the high watermarks. A budget that already
/// saturated additionally waits to drain below the low watermarks, so releasing
/// a single frame does not admit the next one the instant it has room.
fn fits(state: &BulkWriteState, frame_bytes: usize) -> bool {
    let within_high_watermarks = state.admitted_frames < STDIO_RPC_BULK_WRITE_MAX_FRAMES
        && state.admitted_bytes.saturating_add(frame_bytes)
            <= STDIO_RPC_BULK_WRITE_HIGH_WATERMARK_BYTES;
    if !within_high_watermarks {
        return false;
    }
    !state.saturated
        || (state.admitted_frames < STDIO_RPC_BULK_WRITE_LOW_FRAMES
            && state.admitted_bytes < STDIO_RPC_BULK_WRITE_LOW_WATERMARK_BYTES)
}

/// The bounded bulk write budget of one stdout writer.
///
/// Admission stops at the high watermarks — in count and in bytes — and resumes
/// only below the low watermarks, so a saturated budget does not admit the next
/// frame the instant one byte is released.
pub(crate) struct StdioRpcBulkWriteBudget {
    state: Mutex<BulkWriteState>,
    released: Condvar,
}

impl Default for StdioRpcBulkWriteBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl StdioRpcBulkWriteBudget {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(BulkWriteState::default()),
            released: Condvar::new(),
        }
    }

    /// Waits until one frame of [frame_bytes] fits the budget, then admits it.
    ///
    /// The wait is the backpressure on the bulk source: the worker thread that
    /// would produce the next frame blocks here instead of queueing another
    /// frame in front of a control reply.
    pub(crate) fn enter(self: &Arc<Self>, frame_bytes: usize) -> StdioRpcBulkWriteTicket {
        let mut state = self.state.lock().expect("stdio RPC bulk budget lock");
        loop {
            if fits(&state, frame_bytes) {
                state.saturated = false;
                break;
            }
            state.saturated = true;
            state = self
                .released
                .wait(state)
                .expect("stdio RPC bulk budget lock");
        }
        state.admitted_frames += 1;
        state.admitted_bytes = state.admitted_bytes.saturating_add(frame_bytes);
        StdioRpcBulkWriteTicket {
            budget: Arc::clone(self),
            frame_bytes,
        }
    }

    /// Bulk frames admitted and not yet released.
    #[cfg(test)]
    pub(crate) fn admitted_frames(&self) -> usize {
        self.state
            .lock()
            .expect("stdio RPC bulk budget lock")
            .admitted_frames
    }

    /// Bulk bytes admitted and not yet released.
    #[cfg(test)]
    pub(crate) fn admitted_bytes(&self) -> usize {
        self.state
            .lock()
            .expect("stdio RPC bulk budget lock")
            .admitted_bytes
    }
}

/// One admitted bulk frame. Releasing it is what resumes a saturated budget.
pub(crate) struct StdioRpcBulkWriteTicket {
    budget: Arc<StdioRpcBulkWriteBudget>,
    frame_bytes: usize,
}

impl Drop for StdioRpcBulkWriteTicket {
    fn drop(&mut self) {
        let mut state = self
            .budget
            .state
            .lock()
            .expect("stdio RPC bulk budget lock");
        state.admitted_frames = state.admitted_frames.saturating_sub(1);
        state.admitted_bytes = state.admitted_bytes.saturating_sub(self.frame_bytes);
        drop(state);
        self.budget.released.notify_all();
    }
}

/// The single stdout writer shared by every frame producer of this process.
pub(crate) struct StdioRpcWriter<W> {
    inner: Mutex<W>,
    bulk: Arc<StdioRpcBulkWriteBudget>,
}

impl<W: Write> StdioRpcWriter<W> {
    pub(crate) fn new(writer: W) -> Self {
        Self {
            inner: Mutex::new(writer),
            bulk: Arc::new(StdioRpcBulkWriteBudget::new()),
        }
    }

    /// The lane a control reply is written on.
    pub(crate) fn control(&self) -> StdioRpcFrameTarget<'_, W> {
        StdioRpcFrameTarget::new(self, StdioRpcFrameLane::Control)
    }

    /// The lane the request loop writes its own inline replies on.
    pub(crate) fn ordered(&self) -> StdioRpcFrameTarget<'_, W> {
        StdioRpcFrameTarget::new(self, StdioRpcFrameLane::Ordered)
    }

    /// The lane a worker thread writes stream output on.
    pub(crate) fn stream(&self) -> StdioRpcFrameTarget<'_, W> {
        StdioRpcFrameTarget::new(self, StdioRpcFrameLane::Stream)
    }

    /// The target for a lane a caller carries as a value.
    pub(crate) fn lane_target(&self, lane: StdioRpcFrameLane) -> StdioRpcFrameTarget<'_, W> {
        match lane {
            StdioRpcFrameLane::Control => self.control(),
            StdioRpcFrameLane::Ordered => self.ordered(),
            StdioRpcFrameLane::Stream => self.stream(),
        }
    }

    /// The bounded bulk write budget this writer applies.
    #[cfg(test)]
    pub(crate) fn bulk_budget(&self) -> &Arc<StdioRpcBulkWriteBudget> {
        &self.bulk
    }

    /// Recover the underlying writer once every producer released it.
    pub(crate) fn into_writer(self) -> io::Result<W> {
        self.inner
            .into_inner()
            .map_err(|_| io::Error::other("stdio RPC writer lock failed"))
    }

    /// The underlying writer, for tests that inspect what was written.
    #[cfg(test)]
    pub(crate) fn lock_for_test(&self) -> std::sync::MutexGuard<'_, W> {
        self.inner.lock().expect("stdio RPC writer lock")
    }
}

/// One lane of the shared writer, borrowed for a single frame.
///
/// Naming the lane at the write site is the producer-side classification: the
/// compiler refuses a frame written without one.
pub(crate) struct StdioRpcFrameTarget<'a, W> {
    writer: &'a StdioRpcWriter<W>,
    lane: StdioRpcFrameLane,
}

impl<W> Clone for StdioRpcFrameTarget<'_, W> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<W> Copy for StdioRpcFrameTarget<'_, W> {}

impl<'a, W: Write> StdioRpcFrameTarget<'a, W> {
    fn new(writer: &'a StdioRpcWriter<W>, lane: StdioRpcFrameLane) -> Self {
        Self { writer, lane }
    }

    /// Writes one whole bounded JSON line, or reports that it exceeds its bound.
    ///
    /// `Ok(false)` means nothing was written, exactly like
    /// [try_write_stdio_rpc_response]; the caller writes a bounded error frame.
    pub(crate) fn write_frame(self, max_bytes: usize, frame: &Value) -> io::Result<bool> {
        let Some(encoded) = encode_stdio_rpc_frame(frame, max_bytes)? else {
            return Ok(false);
        };
        // Bulk stream output waits for the budget before it takes the writer;
        // control and the request loop's own replies never do.
        let _ticket = match self.lane {
            StdioRpcFrameLane::Stream => Some(self.writer.bulk.enter(encoded.len())),
            StdioRpcFrameLane::Control | StdioRpcFrameLane::Ordered => None,
        };
        let mut writer = self
            .writer
            .inner
            .lock()
            .map_err(|_| io::Error::other("stdio RPC writer lock failed"))?;
        writer.write_all(&encoded)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// A writer that parks inside `write` until the test releases it, so
    /// "inside the writer" is observed rather than inferred from timing.
    ///
    /// The gate is shared with the handle the test keeps, because the writer
    /// itself is inside `write` and its lock cannot be taken to release it.
    #[derive(Clone)]
    struct GatedWriter {
        released: Arc<(Mutex<bool>, Condvar)>,
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl GatedWriter {
        fn new() -> Self {
            Self {
                released: Arc::new((Mutex::new(false), Condvar::new())),
                bytes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn release(&self) {
            let (lock, condvar) = &*self.released;
            *lock.lock().expect("gate lock") = true;
            condvar.notify_all();
        }

        /// Bytes the parked reader consumed while it was parked.
        fn written_len(&self) -> usize {
            self.bytes.lock().expect("bytes lock").len()
        }
    }

    impl Write for GatedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let (lock, condvar) = &*self.released;
            let mut released = lock.lock().expect("gate lock");
            while !*released {
                released = condvar.wait(released).expect("gate lock");
            }
            self.bytes
                .lock()
                .expect("bytes lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn wait_for(predicate: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !predicate() {
            assert!(
                Instant::now() < deadline,
                "condition not reached before its deadline"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn frame(id: &str, filler: usize) -> Value {
        json!({ "id": id, "result": { "text": "x".repeat(filler) } })
    }

    /// Wire bytes of one frame, so a byte assertion is exact to the frame.
    fn encoded_len(frame: &Value) -> usize {
        encode_stdio_rpc_frame(frame, STDIO_RPC_MAX_RESPONSE_BYTES)
            .expect("frame encodes")
            .expect("frame is inside its bound")
            .len()
            + 1
    }

    /// Writes one frame on [lane] from another thread and reports whether it
    /// reached the writer inside [within].
    ///
    /// The deadline is what makes the assertion falsifiable: a lane that waits
    /// for a saturated budget fails the assertion instead of hanging the suite.
    fn lane_write_finishes_within(
        writer: &Arc<StdioRpcWriter<Vec<u8>>>,
        lane: StdioRpcFrameLane,
        id: &str,
        within: Duration,
    ) -> bool {
        let writer = Arc::clone(writer);
        let id = id.to_owned();
        let (finished, reached) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let written = writer
                .lane_target(lane)
                .write_frame(STDIO_RPC_MAX_RESPONSE_BYTES, &frame(&id, 8));
            let _ = finished.send(written.map(|_| ()).map_err(|error| error.to_string()));
        });
        reached.recv_timeout(within).is_ok()
    }

    fn lines(writer: &StdioRpcWriter<Vec<u8>>) -> Vec<String> {
        String::from_utf8(writer.lock_for_test().clone())
            .expect("utf8 frames")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn a_saturated_bulk_budget_never_blocks_control_and_always_blocks_bulk() {
        let writer = Arc::new(StdioRpcWriter::new(Vec::<u8>::new()));
        let budget = Arc::clone(writer.bulk_budget());
        let mut held = Vec::new();
        for _ in 0..STDIO_RPC_BULK_WRITE_MAX_FRAMES {
            held.push(budget.enter(1024));
        }
        assert_eq!(budget.admitted_frames(), STDIO_RPC_BULK_WRITE_MAX_FRAMES);
        // The budget is fully saturated and the control reply still reaches the
        // writer: it is never queued behind bulk, and it consumes no bulk
        // capacity. The request loop's own inline reply behaves the same.
        assert!(
            lane_write_finishes_within(
                &writer,
                StdioRpcFrameLane::Control,
                "control",
                Duration::from_secs(5),
            ),
            "a control reply waits for the bulk budget it exists to interrupt"
        );
        assert!(
            lane_write_finishes_within(
                &writer,
                StdioRpcFrameLane::Ordered,
                "ordered",
                Duration::from_secs(5),
            ),
            "the request loop's own inline reply waits for the bulk budget"
        );
        assert_eq!(budget.admitted_frames(), STDIO_RPC_BULK_WRITE_MAX_FRAMES);
        // A bulk stream frame waits for the budget instead: that wait is the
        // backpressure the bulk source experiences.
        let blocked_writer = Arc::clone(&writer);
        let blocked = std::thread::spawn(move || {
            blocked_writer
                .stream()
                .write_frame(STDIO_RPC_MAX_RESPONSE_BYTES, &frame("bulk", 1024))
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !blocked.is_finished(),
            "the bulk frame waits while the budget is saturated"
        );
        assert_eq!(lines(&writer).len(), 2);
        drop(held);
        blocked
            .join()
            .expect("bulk writer joined")
            .expect("bulk frame is written");
        assert_eq!(budget.admitted_frames(), 0);
        assert_eq!(budget.admitted_bytes(), 0);
        let written = lines(&writer);
        assert_eq!(written.len(), 3);
        assert!(written[0].contains("\"control\""));
        assert!(written[1].contains("\"ordered\""));
        assert!(written[2].contains("\"bulk\""));
        for line in &written {
            serde_json::from_str::<Value>(line).expect("every frame is one whole json line");
        }
    }

    #[test]
    fn producer_bulk_budget_bounds_a_parked_reader() {
        // The reader is parked inside `write`, which is what a paused stdout
        // pipe does to the producer. Eight bulk producers offer 4 MiB each, so
        // the burst the reader is not consuming is 32 MiB: without the budget
        // the producer commits all of it in front of any control reply.
        let gate = GatedWriter::new();
        let writer = Arc::new(StdioRpcWriter::new(gate.clone()));
        let budget = Arc::clone(writer.bulk_budget());
        let filler = 4 * 1024 * 1024 - 1024;
        let one_frame = encoded_len(&frame("bulk-0", filler));
        assert!(
            one_frame < STDIO_RPC_BULK_WRITE_LOW_WATERMARK_BYTES,
            "the test frame must be one bulk frame, got {one_frame} bytes"
        );
        let producers = 8usize;
        let mut threads = Vec::new();
        for index in 0..producers {
            let writer = Arc::clone(&writer);
            threads.push(std::thread::spawn(move || {
                writer.stream().write_frame(
                    STDIO_RPC_MAX_RESPONSE_BYTES,
                    &frame(&format!("bulk-{index}"), filler),
                )
            }));
        }

        wait_for(|| budget.admitted_frames() >= 1);
        std::thread::sleep(Duration::from_millis(200));
        let admitted_frames = budget.admitted_frames();
        let admitted_bytes = budget.admitted_bytes();
        // The reader consumed nothing, so every byte the producer committed is
        // still in front of it. That amount is the bound this change adds.
        assert_eq!(
            gate.written_len(),
            0,
            "the parked reader consumed bytes, the fixture is wrong"
        );
        assert!(
            admitted_frames >= 1,
            "no bulk frame was admitted at all: the write path does not use the \
             bulk budget (removing the budget makes this assertion fail)"
        );
        assert!(
            admitted_frames <= STDIO_RPC_BULK_WRITE_MAX_FRAMES,
            "the producer committed {admitted_frames} bulk frames against a \
             {STDIO_RPC_BULK_WRITE_MAX_FRAMES}-frame budget"
        );
        assert!(
            admitted_bytes <= STDIO_RPC_BULK_WRITE_HIGH_WATERMARK_BYTES,
            "the producer committed {admitted_bytes} bulk bytes against a \
             {STDIO_RPC_BULK_WRITE_HIGH_WATERMARK_BYTES}-byte budget"
        );
        // The count watermark expressed in bytes for this test's frames: at most
        // two 4 MiB frames, against the 32 MiB the producers offered.
        assert!(
            admitted_bytes <= STDIO_RPC_BULK_WRITE_MAX_FRAMES * one_frame,
            "at most {STDIO_RPC_BULK_WRITE_MAX_FRAMES} frames of {one_frame} bytes \
             may be in front of the reader, got {admitted_bytes}"
        );
        assert!(
            admitted_bytes < 8 * 1024 * 1024,
            "two 4 MiB frames bound the committed bulk to 8 MiB, got {admitted_bytes}"
        );

        gate.release();
        let mut written_frames = 0usize;
        for thread in threads {
            if thread
                .join()
                .expect("bulk writer joined")
                .expect("bulk frame is written")
            {
                written_frames += 1;
            }
        }
        // The budget delays the burst, it never drops or duplicates a frame.
        assert_eq!(written_frames, producers);
        assert_eq!(budget.admitted_frames(), 0);
        assert_eq!(budget.admitted_bytes(), 0);
        let written =
            String::from_utf8(gate.bytes.lock().expect("bytes lock").clone()).expect("utf8 frames");
        assert_eq!(written.lines().count(), producers);
        for line in written.lines() {
            serde_json::from_str::<Value>(line).expect("every frame is one whole json line");
        }
    }

    #[test]
    fn a_bulk_ticket_is_held_across_its_whole_write() {
        let gate = GatedWriter::new();
        let writer = Arc::new(StdioRpcWriter::new(gate.clone()));
        let budget = Arc::clone(writer.bulk_budget());
        let entered = Arc::new(AtomicUsize::new(0));
        let writer_for_thread = Arc::clone(&writer);
        let entered_for_thread = Arc::clone(&entered);
        let writing = std::thread::spawn(move || {
            // The gate blocks inside write, so the frame is provably still in
            // the writer while the test inspects the budget.
            let result = writer_for_thread
                .stream()
                .write_frame(STDIO_RPC_MAX_RESPONSE_BYTES, &frame("bulk", 1024));
            entered_for_thread.fetch_add(1, Ordering::SeqCst);
            result
        });
        wait_for(|| budget.admitted_frames() == 1);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            budget.admitted_frames(),
            1,
            "the ticket covers the write, not just the admission"
        );
        assert_eq!(entered.load(Ordering::SeqCst), 0);
        gate.release();
        writing
            .join()
            .expect("bulk writer joined")
            .expect("bulk frame is written");
        assert_eq!(budget.admitted_frames(), 0);
        assert!(
            String::from_utf8(gate.bytes.lock().expect("bytes lock").clone())
                .expect("utf8 frames")
                .contains("\"bulk\"")
        );
    }

    #[test]
    fn the_bulk_budget_stops_at_its_watermarks_and_resumes_below_them() {
        let budget = Arc::new(StdioRpcBulkWriteBudget::new());
        let first = budget.enter(1024);
        let second = budget.enter(1024);
        assert_eq!(budget.admitted_frames(), STDIO_RPC_BULK_WRITE_MAX_FRAMES);
        assert_eq!(budget.admitted_bytes(), 2048);
        let blocked = Arc::clone(&budget);
        let waiting = std::thread::spawn(move || blocked.enter(1024));
        std::thread::sleep(Duration::from_millis(20));
        assert!(!waiting.is_finished());
        drop(second);
        std::thread::sleep(Duration::from_millis(20));
        // One release is still above the low watermark: admission waits for it.
        assert!(!waiting.is_finished());
        drop(first);
        let resumed = waiting.join().expect("waiting bulk writer joined");
        assert_eq!(budget.admitted_frames(), 1);
        drop(resumed);
        assert_eq!(budget.admitted_frames(), 0);
        assert_eq!(budget.admitted_bytes(), 0);
    }

    #[test]
    fn a_maximum_frame_is_admitted_alone_by_the_byte_watermark() {
        let budget = Arc::new(StdioRpcBulkWriteBudget::new());
        let maximum = budget.enter(STDIO_RPC_BULK_WRITE_HIGH_WATERMARK_BYTES);
        assert_eq!(
            budget.admitted_bytes(),
            STDIO_RPC_BULK_WRITE_HIGH_WATERMARK_BYTES
        );
        let blocked = Arc::clone(&budget);
        let waiting = std::thread::spawn(move || blocked.enter(1));
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            !waiting.is_finished(),
            "the byte watermark bounds one maximum frame alone"
        );
        drop(maximum);
        let resumed = waiting.join().expect("waiting bulk writer joined");
        drop(resumed);
        assert_eq!(budget.admitted_bytes(), 0);
    }

    #[test]
    fn concurrent_lanes_keep_every_frame_one_whole_line() {
        let writer = Arc::new(StdioRpcWriter::new(Vec::<u8>::new()));
        let mut threads = Vec::new();
        for index in 0..8 {
            let writer = Arc::clone(&writer);
            threads.push(std::thread::spawn(move || {
                writer
                    .stream()
                    .write_frame(
                        STDIO_RPC_MAX_RESPONSE_BYTES,
                        &frame(&format!("bulk-{index}"), 4096),
                    )
                    .expect("bulk frame is written");
            }));
        }
        for index in 0..2 {
            let writer = Arc::clone(&writer);
            threads.push(std::thread::spawn(move || {
                writer
                    .control()
                    .write_frame(
                        STDIO_RPC_MAX_RESPONSE_BYTES,
                        &frame(&format!("control-{index}"), 8),
                    )
                    .expect("control frame is written");
            }));
        }
        for thread in threads {
            thread.join().expect("frame writer joined");
        }
        let written = lines(&writer);
        assert_eq!(written.len(), 10);
        for line in &written {
            serde_json::from_str::<Value>(line).expect("every frame is one whole json line");
        }
    }

    #[test]
    fn request_methods_classify_their_reply_lane() {
        fn lane_of(method: &str) -> StdioRpcFrameLane {
            let protocol_method =
                licoup_native::contracts::conversation_protocol::ConversationProtocolMethod::from_wire(method)
                    .unwrap_or_else(|| panic!("{method} is a protocol method"));
            let frame = licoup_native::contracts::conversation_protocol::ConversationCommand::frame(
                "request-1",
                "workflow-1",
                protocol_method,
                json!({}),
            );
            let bytes = serde_json::to_vec(&frame).expect("frame serializes");
            parse_stdio_rpc_request(&bytes)
                .unwrap_or_else(|_| panic!("{method} parses"))
                .lane
        }
        for method in [
            "agent.conversation.cancel",
            "agent.conversation.steer",
            "agent.conversation.stop",
            "agent.conversation.force.preview",
            "agent.conversation.force.confirm",
            "agent.conversation.execution.detach",
        ] {
            assert_eq!(
                lane_of(method),
                StdioRpcFrameLane::Control,
                "{method} is in-flight control"
            );
        }
        for method in [
            "agent.conversation.send",
            "agent.conversation.history",
            "catalog.list",
        ] {
            assert_eq!(
                lane_of(method),
                StdioRpcFrameLane::Ordered,
                "{method} is an ordinary reply"
            );
        }
    }
}

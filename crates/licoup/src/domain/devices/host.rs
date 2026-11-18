//! The persistent host's side of the `devices` group: the mailbox receive
//! loop that runs while this device is enrolled, and the local executor of an
//! admitted `agent.message.send`.
//!
//! The loop's two halves own disjoint state. The async half on a
//! multi-threaded runtime owns the `MailboxClient`, keeps the SDK's reconnect
//! semantics — unbounded retry with jittered backoff, terminal on superseded,
//! revoked, device-mismatch or refused — and forwards every DELIVER to the
//! processing thread. The processing thread owns the `DeviceProcessor`
//! (custody and session state are thread-confined `Rc`s). Replies and session
//! dispatches cross back through a request channel whose caller blocks for
//! the send's outcome: the bridged [`Transport`] the SDK's synchronous
//! dispatch boundary needs.
//!
//! A delivery's ACK is sent only after the processor reports `Done`; a
//! `Defer` leaves it stored for the next connection to redeliver.

use anyhow::{Result, anyhow};
use rand_core::{OsRng, RngCore};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::watch;

use licoarc_client::ports::{PortFailure, Transport, TransportOutcome};
use licoarc_client::{Event, HandleToken, MailboxClient, StationError, Verdict};

use super::command::{
    AgentDispatchFailure, DeviceCommand, DeviceCommandExecutor, successful_agent_dispatch,
};
use super::session::device_line;
use super::state::{DevicesState, Peer};
use super::{DeviceIdentity, DeviceProcessor, credentials, devices_dir};

/// The running receive loop; dropping it stops the loop and joins its thread.
pub struct ReceiveLoop {
    stop: watch::Sender<bool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ReceiveLoop {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One forwarded delivery.
struct Delivery {
    sequence: u64,
    handle: HandleToken,
    packet: Vec<u8>,
}

/// Work the processing thread asks the async half to perform against the
/// live connection.
enum OutWork {
    /// The delivery is durably resolved; send its ACK.
    Ack(u64),
    /// One mailbox submit; `reply` carries the mapped port outcome back to
    /// the blocked protocol thread.
    Send {
        handle: HandleToken,
        packet: Vec<u8>,
        reply: SyncSender<Result<TransportOutcome, ()>>,
    },
}

/// Maps one `client.send` result onto the `Transport` port's vocabulary.
fn map_send(result: Result<(), StationError>) -> Result<TransportOutcome, ()> {
    match result {
        Ok(()) => Ok(TransportOutcome::Accepted),
        Err(StationError::Rejected { .. }) => Ok(TransportOutcome::Rejected),
        Err(StationError::Transient { .. }) => Ok(TransportOutcome::Transient),
        Err(StationError::Ambiguous) => Ok(TransportOutcome::Ambiguous),
        Err(_) => Err(()),
    }
}

/// One mailbox send through the async half; `true` when it was accepted.
fn bridged_send(out_tx: &UnboundedSender<OutWork>, handle: HandleToken, packet: &[u8]) -> bool {
    let (reply, rx) = std::sync::mpsc::sync_channel(0);
    if out_tx
        .send(OutWork::Send {
            handle,
            packet: packet.to_vec(),
            reply,
        })
        .is_err()
    {
        return false;
    }
    matches!(rx.recv(), Ok(Ok(TransportOutcome::Accepted)))
}

/// The synchronous session carrier bound to the peer's remote handle: each
/// SDK packet submit blocks the protocol thread until the async half reports
/// the send's outcome.
struct BridgedTransport {
    handle: HandleToken,
    tx: UnboundedSender<OutWork>,
}

impl Transport for BridgedTransport {
    fn submit(&mut self, packet: &[u8]) -> Result<TransportOutcome, PortFailure> {
        let (reply, rx) = std::sync::mpsc::sync_channel(0);
        self.tx
            .send(OutWork::Send {
                handle: self.handle,
                packet: packet.to_vec(),
                reply,
            })
            .map_err(|_| PortFailure::Unavailable)?;
        rx.recv()
            .map_err(|_| PortFailure::Unavailable)?
            .map_err(|_| PortFailure::Unavailable)
    }
}

/// Starts the receive loop when this data home is enrolled.
///
/// The loop is push-driven: the Station delivers, nothing polls. Ordinary
/// connection loss reconnects without an attempt cap; a revoked, superseded
/// or refused mailbox ends the loop, which `devices status` and the next
/// `devices sync` report. An unenrolled home starts nothing.
pub fn start() -> Option<ReceiveLoop> {
    let data_home = licoup_foundation::platform::paths::portable_data_dir().ok()?;
    let dir = devices_dir(&data_home);
    let state = DevicesState::open(&dir).ok()?;
    let enrollment = state.enrollment().ok()??;
    let credentials = credentials(&dir, &enrollment).ok()?;
    let roots = state.ca_path().ok()?.into_iter().collect::<Vec<_>>();
    let line = device_line().ok()?;
    let identity = DeviceIdentity::open(&state.identity_dir()).ok()?;
    let (stop, stopped) = watch::channel(false);
    let thread = std::thread::Builder::new()
        .name("licoup-devices-receive".to_owned())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(run(credentials, roots, dir, line, identity, stopped));
        })
        .ok()?;
    Some(ReceiveLoop {
        stop,
        thread: Some(thread),
    })
}

enum DriveOutcome {
    Stopped,
    Lost,
    Fatal(StationError),
}

/// The async half: connect, forward deliveries, serve the processing
/// thread's send and ack work, reconnect on ordinary loss — the same
/// terminal-error and backoff contract as `licoarc_client::receive_loop`,
/// extended by the outbound lane the pairing and session drains need.
async fn run(
    credentials: licoarc_client::Credentials,
    roots: Vec<PathBuf>,
    dir: PathBuf,
    line: licoarc_client::VerifiedProtocolLine,
    identity: DeviceIdentity,
    mut stop: watch::Receiver<bool>,
) {
    let (in_tx, in_rx) = unbounded_channel::<Delivery>();
    let (out_tx, mut out_rx) = unbounded_channel::<OutWork>();
    let worker = std::thread::Builder::new()
        .name("licoup-devices-process".to_owned())
        .spawn(move || processing_loop(&dir, line, identity, in_rx, out_tx));
    let mut backoff_ms = 250_u64;
    loop {
        if *stop.borrow() {
            break;
        }
        match MailboxClient::connect(&credentials, &roots).await {
            Err(
                error @ (StationError::Superseded
                | StationError::Revoked
                | StationError::DeviceMismatch
                | StationError::Refused),
            ) => {
                log::warn!(
                    "devices receive loop ended: {}",
                    super::station_error_code(&error)
                );
                break;
            }
            Err(_) => {}
            Ok(mut client) => {
                backoff_ms = 250;
                let outcome = drive(&mut client, &in_tx, &mut out_rx, &mut stop).await;
                // A send request consumed just before the connection died must
                // still report back: the blocked protocol thread is released
                // as failed and its lane retries on the next connection.
                while let Ok(work) = out_rx.try_recv() {
                    if let OutWork::Send { reply, .. } = work {
                        let _ = reply.send(Err(()));
                    }
                }
                match outcome {
                    DriveOutcome::Stopped => break,
                    DriveOutcome::Fatal(error) => {
                        log::warn!(
                            "devices receive loop ended: {}",
                            super::station_error_code(&error)
                        );
                        break;
                    }
                    DriveOutcome::Lost => {}
                }
            }
        }
        let wait = jittered(backoff_ms);
        backoff_ms = (backoff_ms * 2).min(60_000);
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = stop.changed() => {
                if *stop.borrow() {
                    break;
                }
            }
        }
    }
    drop(in_tx);
    if let Ok(worker) = worker {
        let _ = worker.join();
    }
}

async fn drive(
    client: &mut MailboxClient,
    in_tx: &UnboundedSender<Delivery>,
    out_rx: &mut UnboundedReceiver<OutWork>,
    stop: &mut watch::Receiver<bool>,
) -> DriveOutcome {
    loop {
        let event = tokio::select! {
            event = client.next() => event,
            work = out_rx.recv() => {
                match work {
                    Some(OutWork::Ack(sequence)) => {
                        if client.ack(sequence).await.is_err() {
                            return DriveOutcome::Lost;
                        }
                    }
                    Some(OutWork::Send { handle, packet, reply }) => {
                        let _ = reply.send(map_send(client.send(handle, &packet).await));
                    }
                    None => return DriveOutcome::Stopped,
                }
                continue;
            }
            _ = stop.changed() => return DriveOutcome::Stopped,
        };
        match event {
            Err(
                error @ (StationError::Superseded
                | StationError::Revoked
                | StationError::DeviceMismatch
                | StationError::Refused),
            ) => {
                return DriveOutcome::Fatal(error);
            }
            Err(_) => return DriveOutcome::Lost,
            Ok(Event::Drained) => {}
            Ok(Event::Deliver {
                sequence,
                handle,
                packet,
            }) => {
                if in_tx
                    .send(Delivery {
                        sequence,
                        handle,
                        packet,
                    })
                    .is_err()
                {
                    return DriveOutcome::Stopped;
                }
            }
        }
    }
}

/// The processing thread: opens the thread-confined processor, retries the
/// pairing replies and outbound work a restart left behind, then handles
/// each forwarded delivery. A deferred delivery is simply not ACKed — the
/// Station redelivers it on the next connection.
fn processing_loop(
    dir: &std::path::Path,
    line: licoarc_client::VerifiedProtocolLine,
    identity: DeviceIdentity,
    mut in_rx: UnboundedReceiver<Delivery>,
    out_tx: UnboundedSender<OutWork>,
) {
    let run = || -> Result<()> {
        let state = DevicesState::open(dir)?;
        let mut processor = DeviceProcessor::open(&state, line, identity, AgentMessageExecutor)?;
        for envelope in processor.pending_replies()? {
            if bridged_send(&out_tx, envelope.handle, &envelope.packet)
                && let Some(pairing_id) = &envelope.pairing_id
            {
                processor.confirm_reply(pairing_id)?;
            }
        }
        for peer in state.peer_rows()? {
            let _ = drain_session(&mut processor, &peer, &out_tx);
        }
        while let Some(delivery) = in_rx.blocking_recv() {
            match processor.accept(delivery.sequence, delivery.handle, &delivery.packet) {
                Verdict::Done => {
                    for envelope in processor.take_outbox() {
                        if bridged_send(&out_tx, envelope.handle, &envelope.packet)
                            && let Some(pairing_id) = &envelope.pairing_id
                        {
                            let _ = processor.confirm_reply(pairing_id);
                        }
                    }
                    for name in processor.dirty_sessions() {
                        if let Ok(Some(peer)) = state.peer_by_session_dir(&name) {
                            let _ = drain_session(&mut processor, &peer, &out_tx);
                        }
                    }
                    let _ = out_tx.send(OutWork::Ack(delivery.sequence));
                }
                Verdict::Defer => {}
            }
        }
        Ok(())
    };
    if let Err(error) = run() {
        log::warn!("devices processor exited: {error}");
    }
}

/// Dispatches one session's committed outbound work through the bridge.
fn drain_session<E: DeviceCommandExecutor>(
    processor: &mut DeviceProcessor<E>,
    peer: &Peer,
    out_tx: &UnboundedSender<OutWork>,
) -> Result<()> {
    let remote =
        HandleToken::parse(&peer.remote_handle).map_err(|_| anyhow!("devices_state_invalid"))?;
    processor.drain_outbound(
        &peer.session_dir,
        BridgedTransport {
            handle: remote,
            tx: out_tx.clone(),
        },
    )
}

fn jittered(base_ms: u64) -> Duration {
    let mut extra = [0_u8; 8];
    OsRng.fill_bytes(&mut extra);
    let jitter = u64::from_be_bytes(extra) % (base_ms / 2 + 1);
    Duration::from_millis(base_ms + jitter)
}

/// Runs an admitted `agent.message.send` through this host's agent port.
#[derive(Default)]
pub struct AgentMessageExecutor;

impl DeviceCommandExecutor for AgentMessageExecutor {
    fn execute(&mut self, _command: &DeviceCommand, mut params: Value) -> Result<Value> {
        let agent = params
            .get("agent")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("agent_binding_required"))?;
        let executable = crate::domain::targets::available_runtime_executable(
            &crate::target_port::agent_target_port(),
            &agent,
        )
        .ok_or_else(|| {
            anyhow::Error::new(AgentDispatchFailure::new(
                "native_agent_runtime_binding_unavailable",
                false,
            ))
        })?;
        params["binaryPath"] = json!(executable.to_string_lossy());
        successful_agent_dispatch(
            crate::agent_port::dispatch("send", &params).map_err(anyhow::Error::new),
        )
    }
}

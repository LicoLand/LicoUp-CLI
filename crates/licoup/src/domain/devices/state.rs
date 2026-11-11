//! The owner-only records of the `devices` group.

use anyhow::{Context, Result, anyhow, ensure};
use licoup_foundation::platform::file_security::{
    atomic_write_private_text, ensure_private_dir, read_private_text_bounded,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use super::command::DeviceCommandLedger;

const RECORD_SCHEMA_VERSION: u32 = 1;
const MAX_RECORD_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Enrollment {
    pub(crate) station: String,
    pub(crate) invitation: String,
    pub(crate) mailbox_id: String,
    pub(crate) enrolled_at: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnrollmentRecord {
    schema_version: u32,
    #[serde(flatten)]
    enrollment: Enrollment,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Peer {
    pub(crate) peer_id: String,
    pub(crate) label: String,
    pub(crate) paired_at: String,
    /// `initiator` or `responder`: which session endpoint this device holds.
    #[serde(default)]
    pub(crate) role: String,
    /// This device's receiving handle for the peer's session records.
    #[serde(default)]
    pub(crate) local_handle: String,
    /// The peer's handle this device sends session records to.
    #[serde(default)]
    pub(crate) remote_handle: String,
    /// The session endpoint directory name under `sessions/`.
    #[serde(default)]
    pub(crate) session_dir: String,
    /// The peer's published identity descriptor (hex fields), as paired.
    #[serde(default)]
    pub(crate) peer_identity: serde_json::Value,
    /// Whether the responder session's first outbound record has bootstrapped.
    #[serde(default)]
    pub(crate) send_bootstrapped: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Pairing {
    pub(crate) pairing_id: String,
    pub(crate) role: String,
    pub(crate) state: String,
    pub(crate) created_at: String,
    /// The handle deliveries for this pairing arrive on: the offerer's
    /// bootstrap handle or the claimer's reply handle.
    #[serde(default)]
    pub(crate) handle: String,
    /// The session endpoint directory name under `sessions/`.
    #[serde(default)]
    pub(crate) session_dir: String,
    /// The claimer's peer: the offerer's identity from its offer (needed to
    /// record the peer once the reply lands).
    #[serde(default)]
    pub(crate) peer_identity: Option<serde_json::Value>,
    /// The label either side supplied, if any.
    #[serde(default)]
    pub(crate) label: Option<String>,
    /// The peer this pairing completed into, once the session was established.
    #[serde(default)]
    pub(crate) peer_id: Option<String>,
    /// The offerer's stored `PairingReply` bytes (base64url), so a redelivered
    /// claim envelope re-answers instead of re-accepting.
    #[serde(default)]
    pub(crate) reply_b64: Option<String>,
    /// The claimer's handle the stored reply is submitted to on redelivery.
    #[serde(default)]
    pub(crate) reply_handle: Option<String>,
    /// Whether the stored reply still awaits a confirmed send: cleared once
    /// the mailbox accepts it, so a crash before the send completes retries.
    #[serde(default)]
    pub(crate) reply_pending: bool,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Table<T> {
    schema_version: u32,
    entries: Vec<T>,
}

pub(crate) struct DevicesState {
    dir: PathBuf,
}

impl DevicesState {
    pub(crate) fn open(dir: &Path) -> Result<Self> {
        ensure_private_dir(dir).context("devices_state_unavailable")?;
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    /// The devices directory itself.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn spool_dir(&self) -> PathBuf {
        self.dir.join("spool")
    }

    /// The directory one session endpoint's durable state lives in.
    pub(crate) fn session_dir(&self, name: &str) -> PathBuf {
        self.dir.join("sessions").join(name)
    }

    /// The device identity directory.
    pub(crate) fn identity_dir(&self) -> PathBuf {
        self.dir.join("identity")
    }

    /// The next monotonically increasing pairing sequence this identity uses.
    ///
    /// The SDK requires every prekey admission on one responder identity to
    /// name a higher sequence than the last; the counter survives restarts in
    /// its own file.
    pub(crate) fn next_pair_sequence(&self) -> Result<u64> {
        let path = self.dir.join("pair-sequence");
        let current = match std::fs::read_to_string(&path) {
            Ok(text) => text
                .trim()
                .parse::<u64>()
                .map_err(|_| anyhow!("devices_state_invalid"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(_) => return Err(anyhow!("devices_state_unavailable")),
        };
        let next = current
            .checked_add(1)
            .filter(|value| *value <= 9_007_199_254_740_991)
            .ok_or_else(|| anyhow!("devices_state_invalid"))?;
        atomic_write_private_text(&path, &next.to_string()).context("devices_state_unavailable")?;
        Ok(next)
    }

    pub(crate) fn ledger(&self) -> Result<DeviceCommandLedger> {
        DeviceCommandLedger::open(&self.dir.join("commands.sqlite3"))
    }

    fn read<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Result<Option<T>> {
        let Some(text) = read_private_text_bounded(&self.dir.join(name), MAX_RECORD_BYTES)
            .context("devices_state_unavailable")?
        else {
            return Ok(None);
        };
        Ok(Some(
            serde_json::from_str(&text).map_err(|_| anyhow!("devices_state_invalid"))?,
        ))
    }

    fn write(&self, name: &str, value: &impl Serialize) -> Result<()> {
        atomic_write_private_text(&self.dir.join(name), &serde_json::to_string_pretty(value)?)
            .context("devices_state_unavailable")
    }

    pub(crate) fn enrollment(&self) -> Result<Option<Enrollment>> {
        let Some(record) = self.read::<EnrollmentRecord>("enrollment.json")? else {
            return Ok(None);
        };
        ensure!(
            record.schema_version == RECORD_SCHEMA_VERSION,
            "devices_state_invalid"
        );
        Ok(Some(record.enrollment))
    }

    pub(crate) fn write_enrollment(&self, enrollment: &Enrollment) -> Result<()> {
        self.write(
            "enrollment.json",
            &EnrollmentRecord {
                schema_version: RECORD_SCHEMA_VERSION,
                enrollment: enrollment.clone(),
            },
        )
    }

    pub(crate) fn write_ca(&self, pem: &str) -> Result<PathBuf> {
        let path = self.dir.join("station-ca.pem");
        atomic_write_private_text(&path, pem).context("devices_state_unavailable")?;
        Ok(path)
    }

    pub(crate) fn remove_ca(&self) -> Result<()> {
        match std::fs::remove_file(self.dir.join("station-ca.pem")) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(anyhow!("devices_state_unavailable")),
        }
    }

    pub(crate) fn ca_path(&self) -> Result<Option<PathBuf>> {
        let path = self.dir.join("station-ca.pem");
        Ok(path.is_file().then_some(path))
    }

    fn table<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Result<Vec<T>> {
        let Some(table) = self.read::<Table<T>>(name)? else {
            return Ok(Vec::new());
        };
        ensure!(
            table.schema_version == RECORD_SCHEMA_VERSION,
            "devices_state_invalid"
        );
        Ok(table.entries)
    }

    fn write_table<T: Serialize>(&self, name: &str, entries: Vec<T>) -> Result<()> {
        self.write(
            name,
            &Table {
                schema_version: RECORD_SCHEMA_VERSION,
                entries,
            },
        )
    }

    pub(crate) fn peers(&self) -> Result<Vec<Value>> {
        Ok(self
            .table::<Peer>("peers.json")?
            .into_iter()
            .map(|peer| {
                json!({ "peerId": peer.peer_id, "label": peer.label, "pairedAt": peer.paired_at })
            })
            .collect())
    }

    pub(crate) fn peer(&self, peer_id: &str) -> Result<Option<Peer>> {
        Ok(self
            .table::<Peer>("peers.json")?
            .into_iter()
            .find(|peer| peer.peer_id == peer_id))
    }

    /// The peer whose session records arrive on `handle` (its local handle).
    pub(crate) fn peer_by_handle(&self, handle: &str) -> Result<Option<Peer>> {
        Ok(self
            .table::<Peer>("peers.json")?
            .into_iter()
            .find(|peer| peer.local_handle == handle))
    }

    /// The peer whose endpoint lives in `session_dir`.
    pub(crate) fn peer_by_session_dir(&self, session_dir: &str) -> Result<Option<Peer>> {
        Ok(self
            .table::<Peer>("peers.json")?
            .into_iter()
            .find(|peer| peer.session_dir == session_dir))
    }

    /// Every peer row (internal; `peers` is the JSON projection).
    pub(crate) fn peer_rows(&self) -> Result<Vec<Peer>> {
        self.table::<Peer>("peers.json")
    }

    /// Inserts or replaces one peer record.
    pub(crate) fn upsert_peer(&self, peer: &Peer) -> Result<()> {
        let mut peers = self.table::<Peer>("peers.json")?;
        peers.retain(|existing| existing.peer_id != peer.peer_id);
        peers.push(peer.clone());
        self.write_table("peers.json", peers)
    }

    #[cfg(test)]
    pub(crate) fn insert_peer(&self, peer: Peer) -> Result<()> {
        let mut peers = self.table::<Peer>("peers.json")?;
        peers.retain(|existing| existing.peer_id != peer.peer_id);
        peers.push(peer);
        self.write_table("peers.json", peers)
    }

    pub(crate) fn remove_peer(&self, peer_id: &str) -> Result<bool> {
        let mut peers = self.table::<Peer>("peers.json")?;
        let before = peers.len();
        peers.retain(|peer| peer.peer_id != peer_id);
        let removed = peers.len() != before;
        if removed {
            self.write_table("peers.json", peers)?;
        }
        Ok(removed)
    }

    pub(crate) fn pairings(&self) -> Result<Vec<Value>> {
        Ok(self
            .table::<Pairing>("pairings.json")?
            .into_iter()
            .map(|pairing| {
                json!({
                    "pairingId": pairing.pairing_id,
                    "role": pairing.role,
                    "state": pairing.state,
                    "createdAt": pairing.created_at,
                })
            })
            .collect())
    }

    /// One pairing row by id.
    pub(crate) fn pairing(&self, pairing_id: &str) -> Result<Option<Pairing>> {
        Ok(self
            .table::<Pairing>("pairings.json")?
            .into_iter()
            .find(|pairing| pairing.pairing_id == pairing_id))
    }

    /// Every pairing row (internal; `pairings` is the JSON projection).
    pub(crate) fn pairing_rows(&self) -> Result<Vec<Pairing>> {
        self.table::<Pairing>("pairings.json")
    }

    /// The completed pairing rows that produced `peer_id`.
    pub(crate) fn pairings_by_peer(&self, peer_id: &str) -> Result<Vec<Pairing>> {
        Ok(self
            .table::<Pairing>("pairings.json")?
            .into_iter()
            .filter(|pairing| pairing.peer_id.as_deref() == Some(peer_id))
            .collect())
    }

    /// The pairing a delivery on `handle` belongs to, if any.
    pub(crate) fn pairing_by_handle(&self, handle: &str) -> Result<Option<Pairing>> {
        Ok(self
            .table::<Pairing>("pairings.json")?
            .into_iter()
            .find(|pairing| pairing.handle == handle))
    }

    /// Inserts or replaces one pairing row.
    pub(crate) fn upsert_pairing(&self, pairing: &Pairing) -> Result<()> {
        let mut pairings = self.table::<Pairing>("pairings.json")?;
        pairings.retain(|existing| existing.pairing_id != pairing.pairing_id);
        pairings.push(pairing.clone());
        self.write_table("pairings.json", pairings)
    }

    #[cfg(test)]
    pub(crate) fn insert_pairing(&self, pairing: Pairing) -> Result<()> {
        let mut pairings = self.table::<Pairing>("pairings.json")?;
        pairings.push(pairing);
        self.write_table("pairings.json", pairings)
    }

    pub(crate) fn remove_pairing(&self, pairing_id: &str) -> Result<bool> {
        let mut pairings = self.table::<Pairing>("pairings.json")?;
        let before = pairings.len();
        pairings.retain(|pairing| pairing.pairing_id != pairing_id);
        let removed = pairings.len() != before;
        if removed {
            self.write_table("pairings.json", pairings)?;
        }
        Ok(removed)
    }
}

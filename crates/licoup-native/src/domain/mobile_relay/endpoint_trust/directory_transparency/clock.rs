use crate::domain::mobile_relay::endpoint_trust::mobile_relay_trust_record_now_epoch;
#[cfg(test)]
use crate::domain::mobile_relay::support::KT_FRESHNESS_NOW_OVERRIDE;
use anyhow::{Result, anyhow};
use time::OffsetDateTime;

pub(in crate::domain::mobile_relay) fn current_secure_mesh_kt_gate_epoch_seconds() -> Result<u64> {
    #[cfg(test)]
    if let Some(now) = KT_FRESHNESS_NOW_OVERRIDE.with(|slot| *slot.borrow()) {
        return Ok(now);
    }
    mobile_relay_trust_record_now_epoch()
}

pub(super) fn epoch_seconds(now: OffsetDateTime) -> Result<u64> {
    u64::try_from(now.unix_timestamp())
        .map_err(|_| anyhow!("mobile relay key transparency clock is before unix epoch"))
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) struct KtFreshnessNowOverrideGuard {
    previous: Option<u64>,
}

#[cfg(test)]
impl Drop for KtFreshnessNowOverrideGuard {
    fn drop(&mut self) {
        KT_FRESHNESS_NOW_OVERRIDE.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn set_kt_freshness_now_override(
    now: u64,
) -> KtFreshnessNowOverrideGuard {
    let previous = KT_FRESHNESS_NOW_OVERRIDE.with(|slot| slot.replace(Some(now)));
    KtFreshnessNowOverrideGuard { previous }
}

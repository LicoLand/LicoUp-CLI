use super::*;

pub(in crate::domain::mobile_relay) fn config_path() -> Result<PathBuf> {
    Ok(ClientStateStore::portable()?
        .root()
        .join("mobile-relay")
        .join("config.json"))
}

pub(in crate::domain::mobile_relay) fn config_path_for_data_home(root: &Path) -> PathBuf {
    crate::platform::client_state::state_root_for_data_home(root)
        .join("mobile-relay")
        .join("config.json")
}

pub(in crate::domain::mobile_relay) fn config_lock_path() -> Result<PathBuf> {
    Ok(ClientStateStore::portable()?
        .root()
        .join("mobile-relay")
        .join("config.writer.lock"))
}

fn kt_authority_reset_guard_path() -> Result<PathBuf> {
    Ok(ClientStateStore::portable()?
        .root()
        .join("mobile-relay")
        .join("secure-mesh-kt-authority-reset.guard"))
}

pub(in crate::domain::mobile_relay) fn kt_authority_reset_in_progress() -> Result<bool> {
    let path = kt_authority_reset_guard_path()?;
    if !private_state_marker_exists(&path)? {
        return Ok(false);
    }
    let raw = read_private_state_marker(&path)?
        .ok_or_else(|| anyhow!("secure mesh KT authority reset guard disappeared"))?;
    let guard: Value = serde_json::from_slice(&raw)
        .map_err(|_| anyhow!("secure mesh KT authority reset guard is invalid"))?;
    ensure!(
        guard.get("schemaVersion").and_then(Value::as_u64)
            == Some(KT_AUTHORITY_RESET_GUARD_SCHEMA_VERSION)
            && guard.get("state").and_then(Value::as_str) == Some(KT_AUTHORITY_RESET_GUARD_STATE),
        "secure mesh KT authority reset guard is invalid"
    );
    Ok(true)
}

pub(in crate::domain::mobile_relay) fn ensure_no_kt_authority_reset_in_progress() -> Result<()> {
    ensure!(
        !kt_authority_reset_in_progress()?,
        "secure mesh KT authority reset is incomplete; security operations remain blocked"
    );
    Ok(())
}

pub(in crate::domain::mobile_relay) fn ensure_secure_mesh_protected_operation_allowed_in()
-> Result<()> {
    ensure_no_kt_authority_reset_in_progress()
}

#[cfg(not(test))]
pub(in crate::domain::mobile_relay) fn kt_authority_reset_failpoint(_name: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn kt_authority_reset_failpoint(name: &str) -> Result<()> {
    KT_AUTHORITY_RESET_FAILPOINT.with(|slot| {
        ensure!(
            slot.borrow().as_ref().copied() != Some(name),
            "secure mesh KT authority reset failpoint"
        );
        Ok(())
    })
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) struct KtAuthorityResetFailpointGuard {
    previous: Option<&'static str>,
}

#[cfg(test)]
impl Drop for KtAuthorityResetFailpointGuard {
    fn drop(&mut self) {
        KT_AUTHORITY_RESET_FAILPOINT.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn set_kt_authority_reset_failpoint(
    name: &'static str,
) -> KtAuthorityResetFailpointGuard {
    let previous = KT_AUTHORITY_RESET_FAILPOINT.with(|slot| slot.replace(Some(name)));
    KtAuthorityResetFailpointGuard { previous }
}

pub(in crate::domain::mobile_relay) fn begin_kt_authority_reset() -> Result<()> {
    let path = kt_authority_reset_guard_path()?;
    let content = serde_json::to_vec(&json!({
        "schemaVersion": KT_AUTHORITY_RESET_GUARD_SCHEMA_VERSION,
        "state": KT_AUTHORITY_RESET_GUARD_STATE
    }))?;
    create_private_state_marker(&path, &content)
        .map_err(|_| anyhow!("secure mesh KT authority reset guard could not be created"))
}

pub(in crate::domain::mobile_relay) fn complete_kt_authority_reset() -> Result<()> {
    let path = kt_authority_reset_guard_path()?;
    ensure!(
        kt_authority_reset_in_progress()?,
        "secure mesh KT authority reset guard is missing"
    );
    ensure!(
        remove_private_state_marker(&path)?,
        "secure mesh KT authority reset guard is missing"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_failpoint_guard_restores_previous_thread_local_state() {
        {
            let _guard = set_kt_authority_reset_failpoint("fixture-reset");
            assert!(kt_authority_reset_failpoint("fixture-reset").is_err());
        }
        assert!(kt_authority_reset_failpoint("fixture-reset").is_ok());
    }
}

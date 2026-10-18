use anyhow::{Result, anyhow};

use super::support::require_text;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshSesameDeviceRecord {
    pub user_id: String,
    pub endpoint_id: String,
    pub active_session_id: Option<String>,
    pub inactive_session_ids: Vec<String>,
    pub revoked: bool,
    pub stale: bool,
}

#[derive(Clone, Debug)]
pub struct SecureMeshSesameSessionManager {
    inactive_session_limit: usize,
    devices: Vec<SecureMeshSesameDeviceRecord>,
}

impl SecureMeshSesameSessionManager {
    pub fn new(inactive_session_limit: usize) -> Self {
        Self {
            inactive_session_limit: inactive_session_limit.max(1),
            devices: Vec::new(),
        }
    }

    pub fn activate_session(
        &mut self,
        user_id: impl Into<String>,
        endpoint_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Result<()> {
        let user_id = require_text(user_id.into(), "user id")?;
        let endpoint_id = require_text(endpoint_id.into(), "endpoint id")?;
        let session_id = require_text(session_id.into(), "session id")?;
        let limit = self.inactive_session_limit;
        let device = self.device_mut_or_insert(&user_id, &endpoint_id);
        if let Some(active_session_id) = &device.active_session_id {
            if active_session_id != &session_id {
                push_bounded_inactive(
                    &mut device.inactive_session_ids,
                    active_session_id.clone(),
                    limit,
                );
            }
        }
        device.active_session_id = Some(session_id);
        device.revoked = false;
        device.stale = false;
        Ok(())
    }

    pub fn mark_session_inactive(
        &mut self,
        user_id: &str,
        endpoint_id: &str,
        session_id: &str,
    ) -> Result<()> {
        let limit = self.inactive_session_limit;
        let device = self.device_mut(user_id, endpoint_id)?;
        if device.active_session_id.as_deref() == Some(session_id) {
            device.active_session_id = None;
        }
        push_bounded_inactive(
            &mut device.inactive_session_ids,
            session_id.to_string(),
            limit,
        );
        Ok(())
    }

    pub fn converge_session_collision(
        &mut self,
        user_id: &str,
        endpoint_id: &str,
        candidate_session_id: &str,
    ) -> Result<String> {
        let candidate_session_id = require_text(candidate_session_id.to_string(), "session id")?;
        let limit = self.inactive_session_limit;
        let device = self.device_mut(user_id, endpoint_id)?;
        let chosen = match &device.active_session_id {
            Some(active) if active <= &candidate_session_id => active.clone(),
            Some(active) => {
                push_bounded_inactive(&mut device.inactive_session_ids, active.clone(), limit);
                candidate_session_id.clone()
            }
            None => candidate_session_id.clone(),
        };
        if chosen != candidate_session_id {
            push_bounded_inactive(
                &mut device.inactive_session_ids,
                candidate_session_id,
                limit,
            );
        }
        device.active_session_id = Some(chosen.clone());
        Ok(chosen)
    }

    pub fn revoke_device(&mut self, user_id: &str, endpoint_id: &str) -> Result<()> {
        let device = self.device_mut(user_id, endpoint_id)?;
        device.active_session_id = None;
        device.inactive_session_ids.clear();
        device.revoked = true;
        device.stale = true;
        Ok(())
    }

    pub fn active_sessions_for_user(&self, user_id: &str) -> Vec<String> {
        self.devices
            .iter()
            .filter(|device| device.user_id == user_id && !device.revoked)
            .filter_map(|device| device.active_session_id.clone())
            .collect()
    }

    pub fn fanout_targets_for_user(&self, user_id: &str) -> Vec<(String, String)> {
        self.devices
            .iter()
            .filter(|device| device.user_id == user_id && !device.revoked)
            .filter_map(|device| {
                device
                    .active_session_id
                    .clone()
                    .map(|session_id| (device.endpoint_id.clone(), session_id))
            })
            .collect()
    }

    pub fn device_record(
        &self,
        user_id: &str,
        endpoint_id: &str,
    ) -> Option<&SecureMeshSesameDeviceRecord> {
        self.devices
            .iter()
            .find(|device| device.user_id == user_id && device.endpoint_id == endpoint_id)
    }

    pub(super) fn device_mut(
        &mut self,
        user_id: &str,
        endpoint_id: &str,
    ) -> Result<&mut SecureMeshSesameDeviceRecord> {
        self.devices
            .iter_mut()
            .find(|device| device.user_id == user_id && device.endpoint_id == endpoint_id)
            .ok_or_else(|| anyhow!("secure mesh Sesame device session record is missing"))
    }

    pub(super) fn device_mut_or_insert(
        &mut self,
        user_id: &str,
        endpoint_id: &str,
    ) -> &mut SecureMeshSesameDeviceRecord {
        if let Some(index) = self
            .devices
            .iter()
            .position(|device| device.user_id == user_id && device.endpoint_id == endpoint_id)
        {
            return &mut self.devices[index];
        }
        self.devices.push(SecureMeshSesameDeviceRecord {
            user_id: user_id.to_string(),
            endpoint_id: endpoint_id.to_string(),
            active_session_id: None,
            inactive_session_ids: Vec::new(),
            revoked: false,
            stale: false,
        });
        self.devices
            .last_mut()
            .expect("secure mesh Sesame device record was inserted")
    }
}

pub(super) fn push_bounded_inactive(values: &mut Vec<String>, value: String, limit: usize) {
    values.retain(|candidate| candidate != &value);
    values.push(value);
    while values.len() > limit {
        values.remove(0);
    }
}

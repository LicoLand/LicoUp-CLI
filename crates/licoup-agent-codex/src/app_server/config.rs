use super::model::ProtocolFailure;
use crate::parser::session::rollout_record_identity;
use serde_json::Value;
use std::path::Path;

pub const MAX_IMAGE_ATTACHMENTS: usize = 4;
pub const SUPPORTED_IMAGE_MEDIA_TYPES: &[&str] =
    &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Canonical ordered local-image input for `turn/start`. The runtime adapter
/// already validated the files; this config parse only maps the request shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalImageInput {
    pub name: String,
    pub media_type: String,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct ProtocolConfig {
    pub prompt: String,
    pub private_instructions: Option<String>,
    pub requested_session_id: String,
    pub session_path: Option<String>,
    pub local_images: Vec<LocalImageInput>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub sandbox: Option<Value>,
    pub approval_policy: Option<Value>,
}

impl ProtocolConfig {
    pub fn from_params(
        params: &Value,
        prompt: &str,
        session_id: &str,
        cwd: Option<&Path>,
    ) -> Result<Self, ProtocolFailure> {
        let session_path = text_param(params, &["sessionPath", "sourcePath"]);
        let requested_session_id = resolve_resume_identity(session_path.as_deref(), session_id)?;

        let sandbox = params
            .get("sandbox")
            .or_else(|| params.get("sandboxMode"))
            .filter(|value| !value.is_null())
            .cloned();
        if let Some(value) = sandbox.as_ref() {
            let valid = value.as_str().is_some_and(|value| {
                matches!(
                    value,
                    "read-only" | "workspace-write" | "danger-full-access"
                )
            });
            if !valid {
                return Err(ProtocolFailure::new(
                    "codex_invalid_sandbox",
                    "The requested Codex sandbox mode is not supported.",
                    "thread/configure",
                ));
            }
        }

        let model = text_param(params, &["model", "modelId"]);
        let reasoning_effort = text_param(params, &["reasoningEffort", "reasoning_effort"])
            .or_else(|| spark_default_reasoning_effort(model.as_deref()));
        let local_images = parse_local_images(params)?;

        Ok(Self {
            prompt: prompt.to_string(),
            private_instructions: text_param(
                params,
                &[
                    "developerInstructions",
                    "privateInstructions",
                    "private_instructions",
                ],
            ),
            requested_session_id,
            session_path,
            local_images,
            cwd: cwd.map(|path| path.to_string_lossy().to_string()),
            model,
            reasoning_effort,
            sandbox,
            approval_policy: params
                .get("approvalPolicy")
                .or_else(|| params.get("approval_policy"))
                .filter(|value| !value.is_null())
                .cloned(),
        })
    }

    pub fn is_resume(&self) -> bool {
        !self.requested_session_id.is_empty() || self.session_path.is_some()
    }
}

fn parse_local_images(params: &Value) -> Result<Vec<LocalImageInput>, ProtocolFailure> {
    let Some(raw) = params.get("attachments") else {
        return Ok(Vec::new());
    };
    if raw.is_null() {
        return Ok(Vec::new());
    }
    let Some(items) = raw.as_array() else {
        return Err(attachment_failure());
    };
    if items.len() > MAX_IMAGE_ATTACHMENTS {
        return Err(attachment_failure());
    }
    let mut images = Vec::with_capacity(items.len());
    for item in items {
        let Some(object) = item.as_object() else {
            return Err(attachment_failure());
        };
        for key in object.keys() {
            if !matches!(key.as_str(), "id" | "name" | "mediaType" | "path") {
                return Err(attachment_failure());
            }
        }
        let name = required_image_field(object, "name")?;
        let media_type = required_image_field(object, "mediaType")?;
        let path = required_image_field(object, "path")?;
        if !SUPPORTED_IMAGE_MEDIA_TYPES.contains(&media_type.as_str()) {
            return Err(attachment_failure());
        }
        images.push(LocalImageInput {
            name,
            media_type,
            path,
        });
    }
    Ok(images)
}

fn required_image_field(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, ProtocolFailure> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(attachment_failure)
}

fn attachment_failure() -> ProtocolFailure {
    ProtocolFailure::new(
        "codex_invalid_local_image",
        "The requested Codex image attachment is not valid.",
        "turn/configure",
    )
}

pub fn spark_default_reasoning_effort(model: Option<&str>) -> Option<String> {
    let model = model?.to_ascii_lowercase();
    model.contains("spark").then(|| "low".to_string())
}

fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn resolve_resume_identity(
    session_path: Option<&str>,
    requested_session_id: &str,
) -> Result<String, ProtocolFailure> {
    let requested_session_id = requested_session_id.trim();
    let Some(session_path) = session_path else {
        return Ok(requested_session_id.to_string());
    };
    let recorded_session_id = rollout_record_identity(Path::new(session_path)).map_err(|_| {
        let mut failure = ProtocolFailure::new(
            "codex_invalid_resume_target",
            "Codex could not read an authoritative conversation identity from the supplied rollout.",
            "thread/resume",
        );
        if !requested_session_id.is_empty() {
            failure.session_id = Some(requested_session_id.to_string());
            failure.thread_id = Some(requested_session_id.to_string());
        }
        failure
    })?;
    if !requested_session_id.is_empty() && requested_session_id != recorded_session_id {
        let mut failure = ProtocolFailure::new(
            "codex_resume_source_identity_mismatch",
            "The supplied Codex rollout belongs to a different conversation.",
            "thread/resume",
        );
        failure.session_id = Some(requested_session_id.to_string());
        failure.thread_id = Some(requested_session_id.to_string());
        return Err(failure);
    }
    Ok(recorded_session_id)
}

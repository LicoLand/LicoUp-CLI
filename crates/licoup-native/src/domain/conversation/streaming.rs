//! JSON-lines serialization of the finalized native-history query projection.

use super::history::CONVERSATION_SCHEMA_VERSION;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::io::Write;

pub(crate) fn conversation_stream(params: &Value) -> Result<()> {
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    stream_to_writer(params, &mut writer)
}

pub(crate) fn stream_to_writer<W: Write>(params: &Value, writer: &mut W) -> Result<()> {
    let listed = super::history::conversation_list(params)?;
    stream_listed_projection(&listed, writer)
}

/// Serialize the same finalized value returned by `conversations list`.
/// Stream owns framing only; identity, lineage, dedupe, paging, cache, runtime,
/// and archive semantics have one owner in the list query.
fn stream_listed_projection<W: Write>(listed: &Value, writer: &mut W) -> Result<()> {
    let agent_id = listed
        .get("agentId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("history_list_projection_invalid"))?;
    let page = listed
        .get("page")
        .cloned()
        .ok_or_else(|| anyhow!("history_list_projection_invalid"))?;
    write_json_line(
        writer,
        &json!({
            "event": "start",
            "ok": true,
            "schemaVersion": CONVERSATION_SCHEMA_VERSION,
            "mode": listed.get("mode").cloned().unwrap_or_else(|| json!("native-history")),
            "scanMode": listed.get("scanMode").cloned().unwrap_or_else(|| json!("browse")),
            "importMode": listed.get("importMode").cloned().unwrap_or_else(|| json!("precise-adapter")),
            "readOnly": true,
            "agentId": agent_id,
            "adapterId": listed.get("adapterId"),
            "adapterLabel": listed.get("adapterLabel"),
            "sources": listed.get("sources"),
            "page": page
        }),
    )?;
    let sessions = listed
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("history_list_projection_invalid"))?;
    for session in sessions {
        write_json_line(
            writer,
            &json!({
                "event": "session",
                "ok": true,
                "agentId": agent_id,
                "session": session
            }),
        )?;
    }
    write_json_line(
        writer,
        &json!({
            "event": "done",
            "ok": true,
            "schemaVersion": CONVERSATION_SCHEMA_VERSION,
            "agentId": agent_id,
            "page": page
        }),
    )
}

fn write_json_line<W: Write>(writer: &mut W, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("lico-conversation-stream-{nonce}"))
    }

    #[test]
    fn stream_is_start_session_done_serialization_of_list() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("sessions.json"),
            br#"{"sessions":[{"sessionId":"one","messages":[{"role":"user","text":"one"}]},{"sessionId":"two","messages":[{"role":"user","text":"two"}]}]}"#,
        )
        .unwrap();
        let params = json!({
            "agent": "opencode",
            "root": root.to_string_lossy(),
            "limit": 1
        });
        let listed = super::super::history::conversation_list(&params).unwrap();
        let mut output = Vec::<u8>::new();
        stream_to_writer(&params, &mut output).unwrap();
        let frames = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0]["event"], "start");
        assert_eq!(frames[1]["session"], listed["sessions"][0]);
        assert_eq!(frames[2]["event"], "done");
        assert_eq!(frames[2]["page"], listed["page"]);
        fs::remove_dir_all(root).unwrap();
    }
}

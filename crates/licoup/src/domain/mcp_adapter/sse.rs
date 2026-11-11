use crate::core::mcp::{DEFAULT_MAX_MESSAGE_BYTES, McpMessage, decode_http_body};
use anyhow::{Result, ensure};

const MAX_SSE_EVENTS: usize = 256;
const MAX_SSE_LINE_BYTES: usize = 256 * 1024;

pub(super) fn decode_sse_messages(body: &[u8]) -> Result<Vec<McpMessage>> {
    ensure!(
        body.len() <= DEFAULT_MAX_MESSAGE_BYTES,
        "mcp_message_too_large"
    );
    let text = std::str::from_utf8(body).map_err(|_| anyhow::anyhow!("mcp_sse_utf8_invalid"))?;
    let mut messages = Vec::new();
    let mut data_lines = Vec::<&str>::new();
    for raw_line in text.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        ensure!(line.len() <= MAX_SSE_LINE_BYTES, "mcp_sse_line_too_large");
        if line.is_empty() {
            dispatch_event(&mut data_lines, &mut messages)?;
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        if field == "data" {
            data_lines.push(value);
        }
    }
    dispatch_event(&mut data_lines, &mut messages)?;
    ensure!(
        messages.len() <= MAX_SSE_EVENTS,
        "mcp_sse_event_limit_exceeded"
    );
    Ok(messages)
}

fn dispatch_event(data_lines: &mut Vec<&str>, messages: &mut Vec<McpMessage>) -> Result<()> {
    if data_lines.is_empty() {
        return Ok(());
    }
    ensure!(
        messages.len() < MAX_SSE_EVENTS,
        "mcp_sse_event_limit_exceeded"
    );
    let data = data_lines.join("\n");
    data_lines.clear();
    if data.is_empty() {
        return Ok(());
    }
    messages.push(decode_http_body(
        data.as_bytes(),
        DEFAULT_MAX_MESSAGE_BYTES,
    )?);
    Ok(())
}

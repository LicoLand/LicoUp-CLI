use std::io::{self, BufRead, BufReader};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::bounds::{
    CONCURRENCY_WAIT, MAX_SSE_DATA_LINES, MAX_SSE_EVENTS_PER_STREAM, MAX_SSE_FRAME_BYTES,
    MAX_SSE_LINE_BYTES, MAX_SSE_STREAMS,
};
use super::concurrency::{BoundedGate, LimitFailure};
use super::http::{self, HttpFailure};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SseFailure {
    Busy,
    EventLimit,
    FrameTooLarge,
    HeadersTooLarge,
    InvalidUtf8,
    InvalidUrl,
    LineTooLarge,
    Request,
    Unavailable,
}

fn stream_gate() -> &'static BoundedGate {
    static GATE: OnceLock<BoundedGate> = OnceLock::new();
    GATE.get_or_init(|| BoundedGate::new(MAX_SSE_STREAMS))
}

pub fn watch_frames<F>(url: &str, stop: &AtomicBool, on_data: F) -> Result<(), SseFailure>
where
    F: FnMut(&str, &[u8]) -> bool,
{
    let url = http::validate_url(url).map_err(map_http_failure)?;
    let _permit = stream_gate()
        .acquire(CONCURRENCY_WAIT)
        .map_err(map_limit_failure)?;
    // SSE is a long-lived stream that idles between events; a short read
    // timeout kills a healthy idle stream. A `timeout_read` is intentionally
    // not configured. Only the connect is time-boxed —
    // liveness after that comes from server heartbeats and the stop flag.
    let request = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .build()
        .get(url.as_str());
    http::record_request_metadata(Some("sse.http"), &request);
    let response = match request.call() {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => {
            http::record_response_metadata(Some("sse.http"), &response);
            return Err(SseFailure::Request);
        }
        Err(_) => return Err(SseFailure::Request),
    };
    http::record_response_metadata(Some("sse.http"), &response);
    http::validate_headers(&response).map_err(map_http_failure)?;

    parse_stream(BufReader::new(response.into_reader()), stop, on_data)
}

pub fn parse_stream<R, F>(
    mut reader: R,
    stop: &AtomicBool,
    mut on_data: F,
) -> Result<(), SseFailure>
where
    R: BufRead,
    F: FnMut(&str, &[u8]) -> bool,
{
    let mut line = Vec::new();
    let mut data = Vec::new();
    let mut frame = Vec::new();
    let mut data_lines = 0usize;
    let mut events = 0usize;
    while !stop.load(Ordering::Relaxed) {
        match read_bounded_line(&mut reader, &mut line)? {
            LineRead::Eof => {
                if !data.is_empty() {
                    dispatch(
                        &mut data,
                        &mut frame,
                        &mut data_lines,
                        &mut events,
                        &mut on_data,
                    )?;
                }
                return Ok(());
            }
            LineRead::Line => {}
        }
        frame.extend_from_slice(&line);
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            if !data.is_empty()
                && !dispatch(
                    &mut data,
                    &mut frame,
                    &mut data_lines,
                    &mut events,
                    &mut on_data,
                )?
            {
                return Ok(());
            }
            frame.clear();
            continue;
        }
        if line.first() == Some(&b':') {
            continue;
        }
        let Some(payload) = line.strip_prefix(b"data:") else {
            continue;
        };
        data_lines = data_lines.saturating_add(1);
        if data_lines > MAX_SSE_DATA_LINES {
            return Err(SseFailure::FrameTooLarge);
        }
        let payload = payload.strip_prefix(b" ").unwrap_or(payload);
        let required = data
            .len()
            .saturating_add(usize::from(!data.is_empty()))
            .saturating_add(payload.len());
        if required > MAX_SSE_FRAME_BYTES {
            return Err(SseFailure::FrameTooLarge);
        }
        if !data.is_empty() {
            data.push(b'\n');
        }
        data.extend_from_slice(payload);
    }
    Ok(())
}

enum LineRead {
    Eof,
    Line,
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    line: &mut Vec<u8>,
) -> Result<LineRead, SseFailure> {
    line.clear();
    loop {
        let available = reader.fill_buf().map_err(map_io_failure)?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(LineRead::Eof)
            } else {
                Ok(LineRead::Line)
            };
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(available.len());
        if line.len().saturating_add(consumed) > MAX_SSE_LINE_BYTES {
            return Err(SseFailure::LineTooLarge);
        }
        let completed = available.get(consumed.saturating_sub(1)) == Some(&b'\n');
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if completed {
            return Ok(LineRead::Line);
        }
    }
}

fn dispatch<F>(
    data: &mut Vec<u8>,
    frame: &mut Vec<u8>,
    data_lines: &mut usize,
    events: &mut usize,
    on_data: &mut F,
) -> Result<bool, SseFailure>
where
    F: FnMut(&str, &[u8]) -> bool,
{
    *events = events.saturating_add(1);
    if *events > MAX_SSE_EVENTS_PER_STREAM {
        return Err(SseFailure::EventLimit);
    }
    let text = std::str::from_utf8(data).map_err(|_| SseFailure::InvalidUtf8)?;
    let keep_watching = on_data(text, frame);
    data.clear();
    frame.clear();
    *data_lines = 0;
    Ok(keep_watching)
}

fn map_io_failure(error: io::Error) -> SseFailure {
    let _ = error;
    SseFailure::Request
}

fn map_limit_failure(failure: LimitFailure) -> SseFailure {
    match failure {
        LimitFailure::Busy => SseFailure::Busy,
        LimitFailure::Unavailable => SseFailure::Unavailable,
    }
}

fn map_http_failure(failure: HttpFailure) -> SseFailure {
    match failure {
        HttpFailure::Busy => SseFailure::Busy,
        HttpFailure::HeadersTooLarge => SseFailure::HeadersTooLarge,
        HttpFailure::InvalidUrl => SseFailure::InvalidUrl,
        HttpFailure::Unavailable => SseFailure::Unavailable,
        _ => SseFailure::Request,
    }
}

/// Shared endpoints also carry unrelated sessions. Only explicit protocol
/// session fields can bind a complete raw frame to the current execution.
pub fn frame_belongs_to_session(data: &str, session_id: &str) -> bool {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
        return false;
    };
    let Some(properties) = event.get("properties") else {
        return false;
    };
    let mut matched = false;
    for object in [
        Some(properties),
        properties.get("info"),
        properties.get("part"),
    ] {
        let Some(object) = object else { continue };
        for key in ["sessionID", "sessionId"] {
            if let Some(id) = object.get(key).and_then(serde_json::Value::as_str) {
                if id != session_id {
                    return false;
                }
                matched = true;
            }
        }
    }
    if event
        .get("type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|kind| kind.starts_with("session."))
        && let Some(id) = properties
            .get("info")
            .and_then(|info| info.get("id"))
            .and_then(serde_json::Value::as_str)
    {
        if id != session_id {
            return false;
        }
        matched = true;
    }
    matched
}

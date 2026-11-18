use anyhow::{Result, anyhow};
use std::net::TcpListener;

pub fn is_reserved(port: u16, reserved: &[u16]) -> bool {
    port == 0 || reserved.contains(&port)
}

pub fn select(
    preferred: u16,
    span: u16,
    reserved: &[u16],
    exhausted_code: &'static str,
) -> Result<u16> {
    select_with(preferred, span, reserved, exhausted_code, |port| {
        TcpListener::bind(("127.0.0.1", port)).is_ok()
    })
}

pub fn select_with<F>(
    preferred: u16,
    span: u16,
    reserved: &[u16],
    exhausted_code: &'static str,
    is_bindable: F,
) -> Result<u16>
where
    F: Fn(u16) -> bool,
{
    let mut candidate = if is_reserved(preferred, reserved) {
        next_non_reserved(preferred.saturating_add(1), reserved)
    } else {
        preferred
    };
    let end = candidate.saturating_add(span);
    while candidate <= end {
        if !is_reserved(candidate, reserved) && is_bindable(candidate) {
            return Ok(candidate);
        }
        candidate = match candidate.checked_add(1) {
            Some(next) => next,
            None => break,
        };
    }
    Err(anyhow!(exhausted_code))
}

fn next_non_reserved(mut port: u16, reserved: &[u16]) -> u16 {
    while is_reserved(port, reserved) {
        match port.checked_add(1) {
            Some(next) => port = next,
            None => return port,
        }
    }
    port
}

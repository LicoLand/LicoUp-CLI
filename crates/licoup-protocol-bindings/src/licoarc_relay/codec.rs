//! Bounded canonical base64url and length-prefixed field primitives.

use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use regex::{Captures, Regex};

use super::constants::{LICOARC_EXPIRES_AT_MAX_CHARS, LICOARC_ID_MAX_CHARS, LICOARC_ID_MIN_CHARS};

pub(in crate::licoarc_relay) fn append_len_prefixed(
    output: &mut Vec<u8>,
    value: &[u8],
) -> Result<()> {
    let length = u16::try_from(value.len())
        .map_err(|_| anyhow!("Lico Arc authenticated field is too large"))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

pub(in crate::licoarc_relay) fn decode_exact_base64url(
    label: &str,
    value: &str,
    expected_bytes: usize,
) -> Result<Vec<u8>> {
    decode_bounded_base64url(label, value, expected_bytes, expected_bytes)
}

pub(in crate::licoarc_relay) fn decode_bounded_base64url(
    label: &str,
    value: &str,
    minimum_bytes: usize,
    maximum_bytes: usize,
) -> Result<Vec<u8>> {
    ensure!(
        minimum_bytes <= maximum_bytes,
        "Lico Arc base64url decoder bounds are invalid"
    );
    let maximum_encoded_len = base64url_encoded_len(maximum_bytes)?;
    ensure!(
        !value.is_empty() && value.len() <= maximum_encoded_len,
        "Lico Arc {label} is outside encoded bounds"
    );
    let decoded = general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .with_context(|| format!("Lico Arc {label} is not base64url"))?;
    ensure!(
        (minimum_bytes..=maximum_bytes).contains(&decoded.len()),
        "Lico Arc {label} is outside decoded bounds"
    );
    ensure!(
        general_purpose::URL_SAFE_NO_PAD.encode(&decoded) == value,
        "Lico Arc {label} is not canonical base64url"
    );
    Ok(decoded)
}

pub(in crate::licoarc_relay) fn base64url_encoded_len(input_bytes: usize) -> Result<usize> {
    let complete = input_bytes
        .checked_div(3)
        .and_then(|groups| groups.checked_mul(4))
        .ok_or_else(|| anyhow!("Lico Arc base64url encoded length overflow"))?;
    let remainder = match input_bytes % 3 {
        0 => 0,
        1 => 2,
        2 => 3,
        _ => unreachable!(),
    };
    complete
        .checked_add(remainder)
        .ok_or_else(|| anyhow!("Lico Arc base64url encoded length overflow"))
}

pub(in crate::licoarc_relay) fn validate_licoarc_id(label: &str, value: &str) -> Result<()> {
    ensure!(
        (LICOARC_ID_MIN_CHARS..=LICOARC_ID_MAX_CHARS).contains(&value.len()),
        "Lico Arc {label} length is outside contract bounds"
    );
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
        "Lico Arc {label} is outside the contract alphabet"
    );
    Ok(())
}

pub(in crate::licoarc_relay) fn validate_expires_at(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= LICOARC_EXPIRES_AT_MAX_CHARS,
        "Lico Arc expiresAt is outside endpoint bounds"
    );
    let captures = rfc3339_pattern()
        .captures(value)
        .ok_or_else(|| anyhow!("Lico Arc expiresAt is not an RFC 3339 date-time"))?;
    let year = capture_number(&captures, "year")?;
    let month = capture_number(&captures, "month")?;
    let day = capture_number(&captures, "day")?;
    let hour = capture_number(&captures, "hour")?;
    let minute = capture_number(&captures, "minute")?;
    let second = capture_number(&captures, "second")?;
    ensure!(
        (1..=12).contains(&month)
            && (1..=days_in_month(year, month)).contains(&day)
            && hour <= 23
            && minute <= 59
            && second <= 60,
        "Lico Arc expiresAt is not an RFC 3339 date-time"
    );
    let offset_minutes = if let Some(sign) = captures.name("offset_sign") {
        let offset_hour = capture_number(&captures, "offset_hour")?;
        let offset_minute = capture_number(&captures, "offset_minute")?;
        ensure!(
            offset_hour <= 23 && offset_minute <= 59,
            "Lico Arc expiresAt is not an RFC 3339 date-time"
        );
        let magnitude = i32::try_from((offset_hour * 60) + offset_minute)
            .map_err(|_| anyhow!("Lico Arc expiresAt offset is outside bounds"))?;
        if sign.as_str() == "-" {
            -magnitude
        } else {
            magnitude
        }
    } else {
        0
    };
    if second == 60 {
        let local_minute = i32::try_from((hour * 60) + minute)
            .map_err(|_| anyhow!("Lico Arc expiresAt time is outside bounds"))?;
        ensure!(
            (local_minute - offset_minutes).rem_euclid(24 * 60) == (24 * 60) - 1,
            "Lico Arc expiresAt leap second is outside the published UTC position"
        );
    }
    Ok(())
}

fn rfc3339_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"^(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})[Tt](?P<hour>\d{2}):(?P<minute>\d{2}):(?P<second>\d{2})(?:\.\d+)?(?P<zone>[Zz]|(?P<offset_sign>[+-])(?P<offset_hour>\d{2}):(?P<offset_minute>\d{2}))$",
        )
        .expect("the static Lico Arc RFC 3339 pattern is valid")
    })
}

fn capture_number(captures: &Captures<'_>, name: &str) -> Result<u32> {
    captures
        .name(name)
        .ok_or_else(|| anyhow!("Lico Arc expiresAt component is missing"))?
        .as_str()
        .parse::<u32>()
        .map_err(|_| anyhow!("Lico Arc expiresAt component is outside numeric bounds"))
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

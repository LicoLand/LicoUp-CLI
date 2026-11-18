//! Balance-detector boundary: quota reads as bound facts.
//!
//! The per-provider sources own acquisition (credentials, lanes, timeouts);
//! this module carries the shared boundary discipline — a payload that names
//! the account it describes is refused, the normalized windows project into
//! the `balance-detector` fact vocabulary, and the read binds to the account
//! the host issued the query for. The bound read rides the in-process
//! snapshot; the wire projection stays on the windows.

use balance_detector::{
    AccountBinding, BalanceFact, BoundBalance, QuotaWindow as BalanceWindow,
    self_asserted_account_field,
};
use serde_json::Value;

use super::contract::{QuotaFetchError, QuotaProvider, QuotaWindow};

/// The account a source knows nothing richer about than "the local lane
/// account behind this host-bound credential or process".
pub(super) const LOCAL_ACCOUNT: &str = "local";

/// The account epoch a source with no rotation signal reports.
pub(super) const FIXED_EPOCH: &str = "1";

/// Refuse a payload that asserts a binding fact itself: a field naming the
/// account, the provider, an epoch, or a credential would let a response
/// write one account's balance onto another's ledger.
pub(super) fn refuse_self_asserted(payload: &Value) -> Result<(), QuotaFetchError> {
    if self_asserted_account_field(payload).is_some() {
        return Err(QuotaFetchError::new("balance_self_asserted"));
    }
    Ok(())
}

/// Project the normalized windows into bound facts and stamp the read with
/// the host's capture time. A malformed fact fails the fetch — dropping it
/// would hide a contract drift behind "provider said nothing".
pub(super) fn bind(
    provider: QuotaProvider,
    account_ref: &str,
    epoch: &str,
    windows: &[QuotaWindow],
    observed_at: &str,
) -> Result<BoundBalance, QuotaFetchError> {
    let facts: Vec<BalanceFact> = windows
        .iter()
        .map(|window| fact(provider, window))
        .collect();
    AccountBinding::new(provider_ref(provider), account_ref, epoch)
        .and_then(|binding| binding.bind(facts, observed_at))
        .map_err(|_| QuotaFetchError::new("balance_binding_invalid"))
}

/// `licoup.<agent-id>` names the provider surface the binding queries.
fn provider_ref(provider: QuotaProvider) -> String {
    format!("licoup.{}", provider.agent_id())
}

/// One UI window in the fact vocabulary: the used share complements to a
/// remaining percent (an over-limit window has zero remaining, never a
/// negative share), and a USD-metered remainder rides as the window's
/// remaining amount.
fn fact(provider: QuotaProvider, window: &QuotaWindow) -> BalanceFact {
    BalanceFact::Quota {
        window: balance_window(provider, &window.label),
        remaining_percent: Some(remaining_percent(window.used_percent)),
        remaining_amount: window.remaining.map(exact_decimal),
        unit: window.remaining.map(|_| "USD".to_owned()),
        resets_at: window.resets_at.clone(),
    }
}

/// Account-level window vocabulary for the well-known labels; every other
/// (per-model, per-budget, provider-named) window stays a provider-namespaced
/// custom period so two surfaces can never collide on `weekly`.
fn balance_window(provider: QuotaProvider, label: &str) -> BalanceWindow {
    match label {
        "weekly" => BalanceWindow::Weekly,
        "session" => BalanceWindow::Session,
        // Cursor's plan window is the monthly billing cycle.
        "plan" | "monthly" => BalanceWindow::Monthly,
        other => BalanceWindow::Custom(format!("{}.{}", provider.agent_id(), slug(other))),
    }
}

/// A namespaced custom-period slug from a display label: `-` separates words
/// so the result parses under the `is_namespaced` grammar (the leading
/// segments carry no underscores).
fn slug(label: &str) -> String {
    let mut slug = String::with_capacity(label.len());
    let mut last_was_separator = true;
    for character in label.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            slug.push('-');
            last_was_separator = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "window".to_owned()
    } else {
        slug.to_owned()
    }
}

/// `100 - used` clamped to the percentage domain: an over-limit window reads
/// as exhausted, not as a negative remainder.
fn remaining_percent(used_percent: f64) -> String {
    exact_decimal((100.0 - used_percent).clamp(0.0, 100.0))
}

/// An exact-decimal rendering of one host-derived value: at most four
/// fractional digits with insignificant zeros stripped.
fn exact_decimal(value: f64) -> String {
    let text = format!("{value:.4}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

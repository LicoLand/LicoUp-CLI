use super::model::ApprovalLedger;
use std::sync::{Mutex, OnceLock};

pub(super) fn ledger() -> &'static Mutex<ApprovalLedger> {
    static LEDGER: OnceLock<Mutex<ApprovalLedger>> = OnceLock::new();
    LEDGER.get_or_init(|| Mutex::new(ApprovalLedger::default()))
}

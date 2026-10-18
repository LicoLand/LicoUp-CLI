//! v7.1 usage source integration tests (V7-U6).
//!
//! These tests exercise the C11 usage-source SDK and the optional analytics
//! package together, against synthetic sources and a synthetic core fact port:
//!
//! - `a34_usage_semantics` — A34: specialist metrics, missing tokens, delta and
//!   cumulative series, resets, replays, corrections, out-of-order delivery,
//!   several sources reporting one call, cached subsets and the split between a
//!   producer's quality and the host's settlement authority.
//! - `a31_uninstall_preservation` — A31 at component level: the package's own
//!   surfaces are really released, the core facts survive, in-flight reads drain
//!   and an installation without the package has nothing to scrape.
//!
//! Everything is synthetic. No account, ledger, usage file, network or real
//! process is involved, and the harness is a double rather than a second ledger.

mod a31_uninstall_preservation;
mod a34_usage_semantics;
mod support;

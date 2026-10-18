//! The dispatch entry's routing decision, answered by the catalogue's policy.
//!
//! The policy itself is owned by `licoup-model-catalog`, beside the candidate
//! vocabulary it composes: routing is a selection concern, and the selection
//! owner sits below both this kernel and the workflow runtime, so neither has
//! to reach into the other for it.
//!
//! This module keeps the established kernel path
//! (`licoup_native::domain::candidate_routing`) as the re-export it already
//! was, and the host still installs its own candidate facts and scope
//! admission here through [`install_candidate_facts`] and
//! [`install_scope_admission`].

pub use licoup_model_catalog::candidate_routing::*;

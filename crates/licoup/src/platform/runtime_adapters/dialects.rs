//! The per-Agent frame-dialect answers this host hands the ACP transport.
//!
//! `licoup-agent-drivers` owns the ACP transport engines and declares the port
//! they read one driver's frame dialect through. This module is the composition
//! above it, and it declares no answer of its own: every Agent on the ACP
//! profile owns its dialect whole, so Copilot, Kimi Code and Hermes each publish
//! their own registration from their own package, and the table in
//! [`super::drivers`] installs those registrations rather than assembling a
//! second copy here. An Agent's transport projection — the client-request and
//! permission-request shapes a frame carries — moved with its dialect, because a
//! projection is only meaningful beside the type it projects.
//!
//! The module is kept as the seam it names rather than deleted: an Agent on this
//! profile that has no package of its own declares its projections here, beside
//! the table that installs its dialect.

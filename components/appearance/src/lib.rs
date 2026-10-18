//! Native converter owner for the appearance slice of the persisted client-state format.
//!
//! This is the package `org.licoland.converter.appearance`: a native converter that
//! declares which published formats it reads and produces, and which program performs
//! the move. It replaces the retired appearance JavaScript codec. There is no Node,
//! Python or interpreter lane left: the package carries a Rust program and nothing else.
//!
//! # One coordinator, one writer
//!
//! The conversion itself is **not** implemented here. The client's migration owner is
//! the single implementation of every domain move, and this package drives it:
//! [`converter::convert`] reads the appearance domain's authority through the client's
//! own read-only projection, and then asks the client's own admission to convert the
//! root. A second implementation here would be a second writer, would duplicate the
//! ledger and marker discipline, and would drift from the client on the first schema
//! change — so there is deliberately none.
//!
//! What this package owns is what a package-owning converter owns:
//!
//! - **The declaration.** [`declaration`] names the published source formats and the
//!   target format through the landed `conversion` contract, and its values are read
//!   from the client's own embedded catalogue rather than restated. A format name this
//!   package claimed on its own would be a second authority for the same fact.
//! - **The native entry.** [`ENTRY`] is the program inside the package payload, and it
//!   is the binary this crate builds. The published converter kind is
//!   `native-executable`; nothing here needs a runtime the package does not carry.
//! - **The refusal vocabulary.** [`converter::Report`] reports the stable code of a
//!   refusal and never reports a domain as converted that the client's owner did not
//!   claim.
//!
//! # What the appearance subject means here
//!
//! The frozen endpoints are the whole persisted-state pair, because that is the only
//! pair the client's catalogue publishes and the only one a migration caller requires.
//! This package owns the appearance domain inside that pair:
//! [`DOMAIN_ID`] (`appearance-presentation`, the store at
//! `client-state/appearance-preferences.json`). A root is converted only when that
//! domain reaches its target; a root whose appearance store is a shape no release
//! published is refused before anything is opened for writing.
//!
//! # Preservation
//!
//! Nothing here truncates, rewrites or removes a source document. The client's owner
//! probes the whole root before it moves anything and commits each domain through its
//! own atomic writer, so a run that stops half way leaves the source readable and the
//! next run reconciles the committed store instead of repeating the move.

pub mod converter;
pub mod declaration;

/// The package identity this converter is published under.
pub const PACKAGE_ID: &str = "org.licoland.converter.appearance";

/// The package version, taken from the crate so the two cannot disagree.
pub const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The published display name.
pub const DISPLAY_NAME: &str = "LicoUp appearance resource converter";

/// The converter entry inside the package payload.
///
/// Relative to the package root, and exactly the binary this crate builds. The
/// conversion contract refuses an entry that is not a relative path inside the payload,
/// so the value is a published fact rather than a local convention.
pub const ENTRY: &str = "bin/licoup-appearance-convert";

/// The one client version line this converter is admitted by.
///
/// Which client builds may load a package and which formats it owns are separate
/// claims; this is the first, and it takes no part in identifying the second.
pub const CLIENT_COMPATIBILITY: &str = ">=0.3.0, <1.0.0";

/// The host protocol range the package needs.
pub const HOST_PROTOCOL: (u32, u32) = (1, 0);

/// The profile the package declares itself under.
pub const PROFILE_ID: &str = "native-converter";

/// The migration domain this converter owns the appearance subject of.
pub const DOMAIN_ID: &str = "appearance-presentation";

/// The schema of the one JSON report every invocation prints.
pub const REPORT_SCHEMA: &str = "licoup.appearance-conversion-report.v1";

/// The document the package's published manifest is committed as, relative to the
/// component root.
pub const MANIFEST_FILE: &str = "package/manifest.json";

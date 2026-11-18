//! The untrusted-extension process boundary.
//!
//! C09 says an extension is a separate program. The host module built the host half of that
//! contract — catalog, generations, admission, drain — over a carrier *port*
//! whose in-process implementations cannot confine anything. This module is the
//! other half: a real subprocess carrier with real operating-system limits, and
//! an honest report of the dimensions this platform cannot enforce.
//!
//! Five rules are the whole design, and each is enforced by construction:
//!
//! 1. **Two modes, never a silent third.** [`IsolationMode::TrustedLocal`] runs
//!    a program the user supplied with no filesystem or network confinement and
//!    says so in its record; [`IsolationMode::Restricted`] refuses to start at
//!    all unless this platform really enforces the confinement the mode claims
//!    ([`PlatformConfinement`]). Nothing here downgrades a restricted request
//!    into a trusted run to "make it work".
//! 2. **A restricted instance is one process.** The restricted profile denies
//!    `process-fork` — which on this host also denies fork, vfork, `posix_spawn`
//!    and everything built on them — so no descendant can exist that the
//!    supervised wait would not account for, and the per-process POSIX limits
//!    really bound the instance. A program that declares it needs descendants is
//!    refused before anything is created. A trusted local program may have
//!    descendants: they are killed only while they stay in its process group, and
//!    its release therefore covers the supervised process and that group, not the
//!    whole instance ([`ReleaseScope::ProcessGroup`]). The owner of such a release
//!    stays unverified rather than being cleared on a root wait; a closed stdout
//!    pipe is residue evidence and never a proof of absence.
//! 3. **A declaration is the granted envelope, not the manifest's request.** The
//!    resource roots, network grant and resource limits an instance runs under
//!    are decided by the composition, validated here (roots must stay inside the
//!    managed root, the executable may not live in a writable root), and
//!    recorded as soon as the process exists — before the handshake, so a record
//!    that cannot be written stops the instance instead of leaving an unrecorded
//!    process running ([`IsolationLedger`]). A restricted program may declare
//!    its own environment but not the variables the host owns — home, temporary
//!    directory, working directory, path, loader and interpreter control —
//!    because those *are* the envelope; declaring one is refused before anything
//!    is created instead of being ignored or silently overridden. A widened
//!    manifest cannot retroactively widen a recorded grant.
//! 4. **Only what the OS enforces is claimed.** CPU seconds, file bytes, open
//!    files and the supervised process group come from POSIX rlimits and
//!    process-group teardown; filesystem scopes and the network denial come from
//!    macOS Seatbelt; address space is *not* claimed on a platform that does not
//!    enforce it. [`PlatformConfinement::detect`] reports each dimension, and a
//!    restricted run that asks for an unenforceable limit is refused rather than
//!    silently downgraded. What a trusted local run gets is a process *group*,
//!    not a whole tree: a descendant that leaves the group with `setsid`/`setpgid`
//!    is not reclaimed or sandboxed. The release remains unverified regardless
//!    of whether that descendant retains the instance's stdout pipe.
//! 5. **The record is a boundary record, not a second effect ledger.** A release
//!    is recorded only after the carrier observed the process exit
//!    ([`ObservedExit`]), together with the scope it really covers
//!    ([`ReleaseScope`]) and the limits' real scope ([`LimitScope`]); cancellation
//!    is a request that never settles anything by itself; an unsettled call after
//!    a process death is reported unknown and never re-dispatched. Withdrawing a
//!    grant stops *new* admission and never claims an already-performed effect was
//!    retracted. A genuinely unterminated
//!    tail is terminated before the next append, so no later record is ever
//!    concatenated onto a damaged one; a damaged complete line fails closed
//!    instead of being skipped.
//!
//! What is deliberately absent: no `catch_unwind` isolation (release profiles
//! build with `panic = "abort"`, so a fault in-process is a host fault), no
//! `sh` wrapper presented as a sandbox, no interpreter bundled by the core (the
//! program and its runtime are resolved by [`ProgramSource`], which the
//! composition owns), and no durability claim for a managed root that does not
//! exist. The carrier composes the existing supervision instead of owning a
//! second process account: process-group spawn and teardown come from
//! `crate::platform::process_supervisor`, the journal from the host's
//! `RuntimeCatalogJournal`, and the session-owner fact from
//! `SessionOwner`.
//!
//! Platform boundary, stated plainly: filesystem and network confinement are
//! implemented for macOS Seatbelt. Where the platform has no such control, the
//! dimension is reported `Unavailable` and restricted mode is refused; trusted
//! local mode still runs the user's own program, visibly unconfined. POSIX
//! resource limits and process-group teardown apply on Unix. No Windows claim is
//! made by this build.

mod capability;
mod carrier;
mod confinement;
mod declaration;
mod limits;
mod package_program;
mod program;

pub use capability::{IsolationMode, PlatformConfinement, Support};
pub use carrier::{InstanceFacts, IsolatedProcessCarrier, IsolationPolicy};
pub use declaration::{
    IsolationLedger, IsolationRecord, LimitScope, NetworkGrant, ObservedExit, ReleaseScope,
    ResourceDeclaration, RevocationReason,
};
pub use limits::{EnforcedLimits, ResourceLimits};
pub use package_program::PackagePrograms;
pub use program::{ProgramSource, ResolvedProgram, StaticPrograms};

pub(crate) use program::program_unavailable;

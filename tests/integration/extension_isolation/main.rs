//! Process isolation at `component-integration` level: real untrusted-extension
//! processes, real operating-system limits, and an honest record of both.
//!
//! This harness starts real programs — a synthetic shell extension, the
//! published reference SDK sample, and a connection probe — under the
//! production `IsolatedProcessCarrier`, driven through the production
//! `ExtensionHost` where the host's own rules are the thing under test. It
//! asserts observable facts only: process liveness measured with signal 0,
//! bytes read from a real pipe, exit statuses the kernel reported, and records
//! written to a real managed root.
//!
//! What is synthetic, stated plainly: the packages, roots, markers and ports
//! belong to each test, created under a temporary directory. Nothing here
//! touches a user file, an existing process, a credential, an account or a
//! remote service.
//!
//! What the levels do not prove: a sandboxed platform is not a supported
//! platform. Where this build has no control (address space on Darwin, any
//! confinement on an unimplemented platform) the carrier reports `Unavailable`
//! and restricted mode is refused; the suite asserts that correspondence
//! instead of pretending the control exists.

mod a19_process_isolation;
mod a39_permissions_and_quota;
mod boundary_verification;
mod codex_package_turn;
mod creation_paths;
mod package_execution;
mod silent_writer;
mod support;

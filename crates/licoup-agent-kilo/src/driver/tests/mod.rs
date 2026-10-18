//! This package's own half of the Kilo Code driver seam.
//!
//! This suite states what the package owns: the driver declaration the host
//! composition reads, the launch shape, the capability probe composed in the
//! host's vocabulary, and the turn's failure projection onto the host's failure
//! type. The Agent's remaining half — the request shape, the stream
//! classification, the projection and the parser — is stated beside the code it
//! belongs to. The host's own answer for this package's ports, and the force-stop
//! descriptor that reads the same policy, are stated by the host's suite, because
//! they are the host's.
//!
//! These claims moved here with the code they drive; they drove the same
//! functions through the host's former Kilo module, so nothing about what they
//! establish changed with the path they are reached by.

mod composition;
mod execution;
mod probe;

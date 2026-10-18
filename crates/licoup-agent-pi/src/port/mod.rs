//! The ports this package asks its host to answer.
//!
//! A package is one Agent's program and it is not the client: everything it
//! cannot decide for itself arrives here. [`turn_event`] is where one Pi turn's
//! events go, because the client owns the consumer; [`execution`] is what the
//! extension host answers when it starts this package's binary, and it carries
//! the dispatch and admission facts that belong to the client.

pub mod execution;
pub mod turn_event;

//! Stable composition root for bounded native-agent SQLite history parsing.

pub(super) mod codec;
mod composition;
mod cursor;
mod cursor_cli;
mod cursor_projection;
mod fallback;
mod openagent;

pub(crate) use composition::{
    CopilotChatSessionsReadError, copilot_chat_sessions_document, parse_sqlite_sessions,
};
pub(crate) use cursor::cursor_composer_catalog;

#[cfg(test)]
mod tests;

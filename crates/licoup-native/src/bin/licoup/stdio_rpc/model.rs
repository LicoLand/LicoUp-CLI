use super::*;
use licoup_native::ffi::generated::client_state::{ClientStateGetRequest, ClientStateSetRequest};

pub(crate) enum StdioRpcLine {
    Eof,
    Request(Vec<u8>),
    TooLarge,
}

#[derive(Debug)]
pub(crate) enum StdioRpcMethod {
    Execute {
        args: Vec<String>,
        portable_data_dir: Option<PathBuf>,
    },
    Conversation {
        operation: String,
        params: Value,
        portable_data_dir: Option<PathBuf>,
    },
    ClientConversation {
        params: Value,
        portable_data_dir: Option<PathBuf>,
    },
    StrategyExecute {
        params: Value,
        portable_data_dir: Option<PathBuf>,
    },
    /// One validated `selection.*` request: the projection of the selection
    /// facts and the transitions over the durable selection policy.
    Selection {
        request: SelectionRequest,
        portable_data_dir: Option<PathBuf>,
    },
    Catalog {
        operation: String,
        params: Value,
        portable_data_dir: Option<PathBuf>,
    },
    StateGet {
        request: ClientStateGetRequest,
        portable_data_dir: Option<PathBuf>,
    },
    StateSet {
        request: ClientStateSetRequest,
        portable_data_dir: Option<PathBuf>,
    },
    Shutdown {
        params: Value,
    },
    DataHomeRelocate {
        params: Value,
    },
    DataHomeStatus,
    DataHomeRecover {
        params: Value,
    },
    DataHomeCleanup {
        params: Value,
    },
}

#[derive(Debug)]
pub(crate) struct StdioRpcRequest {
    pub(crate) id: String,
    pub(crate) workflow_id: String,
    pub(crate) method: StdioRpcMethod,
    /// The lane this request's own reply is written on: `Control` for an
    /// in-flight control request (the schema's `inFlightControl` methods),
    /// `Ordered` for every other reply the request loop writes inline. Frames a
    /// worker thread writes for this request are classified `Stream` at the
    /// spawn site.
    pub(crate) lane: StdioRpcFrameLane,
}

#[derive(Debug)]
pub(crate) struct StdioRpcRequestError {
    pub(crate) id: Option<String>,
    pub(crate) workflow_id: Option<String>,
    pub(crate) code: &'static str,
}

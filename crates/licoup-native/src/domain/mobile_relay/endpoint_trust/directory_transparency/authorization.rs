mod exact;
mod local;
mod peer;

pub(in crate::domain::mobile_relay) use exact::authorize_exact_local_directory_response;
pub(in crate::domain::mobile_relay) use local::authorize_local_pairwise_directory;
pub(in crate::domain::mobile_relay) use peer::{
    authorize_peer_pairwise_directory, authorize_peer_pairwise_directory_for_purpose,
};

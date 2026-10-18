use openmls::prelude::MlsGroup;
use zeroize::Zeroizing;

use crate::core::secure_mesh_mls_pq_epoch::SecureMeshMlsMlKem1024EpochExtension;

pub struct SecureMeshMlsWelcome {
    pub commit_message: Vec<u8>,
    pub welcome_message: Vec<u8>,
}

pub struct SecureMeshMlsCommit {
    pub commit_message: Vec<u8>,
    pub welcome_message: Option<Vec<u8>>,
}

pub struct SecureMeshMlsGroup {
    pub(super) group: MlsGroup,
    pub(super) authenticated_group_context: Vec<u8>,
    pub(super) mlkem1024_epoch_extension: Option<SecureMeshMlsMlKem1024EpochExtension>,
    pub(super) mlkem1024_epoch_secret: Option<Zeroizing<[u8; 32]>>,
}

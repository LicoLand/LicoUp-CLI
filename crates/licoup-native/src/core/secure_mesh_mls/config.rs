use openmls::prelude::{Ciphersuite, MlsGroupCreateConfig, MlsGroupJoinConfig};

use super::capability_extension::{
    SecureMeshMlsCapabilityExtension, secure_mesh_mls_group_context_extensions,
    secure_mesh_mls_leaf_capabilities,
};

pub fn secure_mesh_mls_ciphersuite() -> Ciphersuite {
    Ciphersuite::MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519
}

pub(super) fn secure_mesh_mls_create_config() -> MlsGroupCreateConfig {
    MlsGroupCreateConfig::builder()
        .ciphersuite(secure_mesh_mls_ciphersuite())
        .use_ratchet_tree_extension(true)
        .with_group_context_extensions(
            secure_mesh_mls_group_context_extensions(
                &SecureMeshMlsCapabilityExtension::awaiting_member_negotiation(),
            )
            .expect("secure mesh MLS built-in capability extension must be valid"),
        )
        .capabilities(secure_mesh_mls_leaf_capabilities())
        .build()
}

pub(super) fn secure_mesh_mls_join_config() -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder()
        .use_ratchet_tree_extension(true)
        .build()
}

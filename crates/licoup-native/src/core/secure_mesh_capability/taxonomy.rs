use anyhow::{Result, anyhow};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SecurityCapability {
    AuthenticatedEncryption,
    CompleteAadBinding,
    EndpointIdentityAuthentication,
    VerifyBeforeSend,
    ReplayDuplicateRejection,
    ExpiryRollbackRejection,
    RatchetForwardSecrecy,
    EncryptedRelayHeaders,
    AuthenticatedPadding,
    PlaintextFallbackForbidden,
    SecureSessionFoundation,
    MemoryOnlyEphemeral,
    OsSecureStore,
    SoftwareBacked,
    NonExportable,
    DeviceBound,
    UnlockedDeviceRequired,
    OsUserPresence,
    DeviceCredential,
    StrongBiometric,
    AuthenticationValidityWindow,
    EnrollmentChangeInvalidation,
    HardwareBacked,
    HardwareEnforcedUserAuthentication,
    AndroidKeystore,
    AppleKeychain,
    LinuxSecretService,
    DataProtectionKeychain,
    Tee,
    Strongbox,
    SecureEnclave,
}

impl SecurityCapability {
    pub const COUNT: usize = 31;
    pub const ALL: [Self; Self::COUNT] = [
        Self::AuthenticatedEncryption,
        Self::CompleteAadBinding,
        Self::EndpointIdentityAuthentication,
        Self::VerifyBeforeSend,
        Self::ReplayDuplicateRejection,
        Self::ExpiryRollbackRejection,
        Self::RatchetForwardSecrecy,
        Self::EncryptedRelayHeaders,
        Self::AuthenticatedPadding,
        Self::PlaintextFallbackForbidden,
        Self::SecureSessionFoundation,
        Self::MemoryOnlyEphemeral,
        Self::OsSecureStore,
        Self::SoftwareBacked,
        Self::NonExportable,
        Self::DeviceBound,
        Self::UnlockedDeviceRequired,
        Self::OsUserPresence,
        Self::DeviceCredential,
        Self::StrongBiometric,
        Self::AuthenticationValidityWindow,
        Self::EnrollmentChangeInvalidation,
        Self::HardwareBacked,
        Self::HardwareEnforcedUserAuthentication,
        Self::AndroidKeystore,
        Self::AppleKeychain,
        Self::LinuxSecretService,
        Self::DataProtectionKeychain,
        Self::Tee,
        Self::Strongbox,
        Self::SecureEnclave,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::AuthenticatedEncryption => "protocol.authenticated_encryption",
            Self::CompleteAadBinding => "protocol.complete_aad_binding",
            Self::EndpointIdentityAuthentication => "protocol.endpoint_identity_authentication",
            Self::VerifyBeforeSend => "protocol.verify_before_send",
            Self::ReplayDuplicateRejection => "protocol.replay_duplicate_rejection",
            Self::ExpiryRollbackRejection => "protocol.expiry_rollback_rejection",
            Self::RatchetForwardSecrecy => "protocol.ratchet_forward_secrecy",
            Self::EncryptedRelayHeaders => "protocol.encrypted_relay_headers",
            Self::AuthenticatedPadding => "protocol.authenticated_padding",
            Self::PlaintextFallbackForbidden => "protocol.plaintext_fallback_forbidden",
            Self::SecureSessionFoundation => "protocol.secure_session_foundation",
            Self::MemoryOnlyEphemeral => "custody.memory_only_ephemeral",
            Self::OsSecureStore => "custody.os_secure_store",
            Self::SoftwareBacked => "custody.software_backed",
            Self::NonExportable => "custody.non_exportable",
            Self::DeviceBound => "custody.device_bound",
            Self::UnlockedDeviceRequired => "custody.unlocked_device_required",
            Self::OsUserPresence => "custody.os_user_presence",
            Self::DeviceCredential => "custody.device_credential",
            Self::StrongBiometric => "custody.strong_biometric",
            Self::AuthenticationValidityWindow => "custody.authentication_validity_window",
            Self::EnrollmentChangeInvalidation => "custody.enrollment_change_invalidation",
            Self::HardwareBacked => "custody.hardware_backed",
            Self::HardwareEnforcedUserAuthentication => {
                "custody.hardware_enforced_user_authentication"
            }
            Self::AndroidKeystore => "custody.android_keystore",
            Self::AppleKeychain => "custody.apple_keychain",
            Self::LinuxSecretService => "custody.linux_secret_service",
            Self::DataProtectionKeychain => "custody.data_protection_keychain",
            Self::Tee => "custody.tee",
            Self::Strongbox => "custody.strongbox",
            Self::SecureEnclave => "custody.secure_enclave",
        }
    }

    pub fn from_id(id: &str) -> Result<Self> {
        match id {
            "protocol.authenticated_encryption" => Ok(Self::AuthenticatedEncryption),
            "protocol.complete_aad_binding" => Ok(Self::CompleteAadBinding),
            "protocol.endpoint_identity_authentication" => Ok(Self::EndpointIdentityAuthentication),
            "protocol.verify_before_send" => Ok(Self::VerifyBeforeSend),
            "protocol.replay_duplicate_rejection" => Ok(Self::ReplayDuplicateRejection),
            "protocol.expiry_rollback_rejection" => Ok(Self::ExpiryRollbackRejection),
            "protocol.ratchet_forward_secrecy" => Ok(Self::RatchetForwardSecrecy),
            "protocol.encrypted_relay_headers" => Ok(Self::EncryptedRelayHeaders),
            "protocol.authenticated_padding" => Ok(Self::AuthenticatedPadding),
            "protocol.plaintext_fallback_forbidden" => Ok(Self::PlaintextFallbackForbidden),
            "protocol.secure_session_foundation" => Ok(Self::SecureSessionFoundation),
            "custody.memory_only_ephemeral" => Ok(Self::MemoryOnlyEphemeral),
            "custody.os_secure_store" => Ok(Self::OsSecureStore),
            "custody.software_backed" => Ok(Self::SoftwareBacked),
            "custody.non_exportable" => Ok(Self::NonExportable),
            "custody.device_bound" => Ok(Self::DeviceBound),
            "custody.unlocked_device_required" => Ok(Self::UnlockedDeviceRequired),
            "custody.os_user_presence" => Ok(Self::OsUserPresence),
            "custody.device_credential" => Ok(Self::DeviceCredential),
            "custody.strong_biometric" => Ok(Self::StrongBiometric),
            "custody.authentication_validity_window" => Ok(Self::AuthenticationValidityWindow),
            "custody.enrollment_change_invalidation" => Ok(Self::EnrollmentChangeInvalidation),
            "custody.hardware_backed" => Ok(Self::HardwareBacked),
            "custody.hardware_enforced_user_authentication" => {
                Ok(Self::HardwareEnforcedUserAuthentication)
            }
            "custody.android_keystore" => Ok(Self::AndroidKeystore),
            "custody.apple_keychain" => Ok(Self::AppleKeychain),
            "custody.linux_secret_service" => Ok(Self::LinuxSecretService),
            "custody.data_protection_keychain" => Ok(Self::DataProtectionKeychain),
            "custody.tee" => Ok(Self::Tee),
            "custody.strongbox" => Ok(Self::Strongbox),
            "custody.secure_enclave" => Ok(Self::SecureEnclave),
            _ => Err(anyhow!("unknown secure mesh capability identifier")),
        }
    }

    pub(super) const fn index(self) -> usize {
        self as usize
    }
}

impl Serialize for SecurityCapability {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for SecurityCapability {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let id = String::deserialize(deserializer)?;
        Self::from_id(&id).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityScope {
    ProtocolSession,
    LocalCustody,
}

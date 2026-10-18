export function createCapabilityFacts(payload) {
  const standardReady = probeSucceeded(payload.standardKeychain);
  const dataProtectionReady = probeSucceeded(payload.dataProtectionKeychain);
  const osStoreReady = standardReady || dataProtectionReady;
  const deviceOnlyReady =
    payload.standardKeychain?.deviceOnlyAccessibilityObserved === true ||
    payload.dataProtectionKeychain?.deviceOnlyAccessibilityObserved === true;
  const userPresenceReady = userPresenceOperationSucceeded(payload);
  const secureEnclaveReady = payload.secureEnclaveOperationSucceeded === true;

  return [
    capabilityFact(
      "custody.os_secure_store",
      osStoreReady,
      "macos_keychain_create_read_delete_verified",
      "macos_keychain_operation_unavailable",
    ),
    capabilityFact(
      "custody.software_backed",
      osStoreReady,
      "macos_keychain_software_custody_available",
      "macos_keychain_operation_unavailable",
    ),
    capabilityFact(
      "custody.non_exportable",
      secureEnclaveReady,
      "secure_enclave_private_key_non_exportable_operation_verified",
      "non_exportable_private_key_operation_unverified",
      secureEnclaveReady ? "supported" : "unverified",
    ),
    capabilityFact(
      "custody.device_bound",
      deviceOnlyReady || secureEnclaveReady,
      "this_device_only_or_secure_enclave_verified",
      "device_bound_operation_unverified",
      deviceOnlyReady || secureEnclaveReady ? "supported" : "unverified",
    ),
    capabilityFact(
      "custody.unlocked_device_required",
      deviceOnlyReady,
      "when_unlocked_this_device_only_verified",
      "unlocked_device_constraint_unverified",
      deviceOnlyReady ? "supported" : "unverified",
    ),
    capabilityFact(
      "custody.os_user_presence",
      userPresenceReady,
      "keychain_user_presence_operation_verified",
      payload.localAuthenticationAvailable === true
        ? "user_presence_operation_not_verified"
        : "local_authentication_unavailable",
      userPresenceReady
        ? "supported"
        : payload.localAuthenticationAvailable === true
          ? "unverified"
          : "unsupported",
    ),
    capabilityFact(
      "custody.device_credential",
      userPresenceReady && payload.localAuthenticationAvailable === true,
      "device_owner_authentication_operation_verified",
      payload.localAuthenticationAvailable === true
        ? "device_credential_operation_not_verified"
        : "device_credential_unavailable",
      userPresenceReady
        ? "supported"
        : payload.localAuthenticationAvailable === true
          ? "unverified"
          : "unsupported",
    ),
    capabilityFact(
      "custody.strong_biometric",
      false,
      "strong_biometric_constraint_verified",
      payload.biometricAuthenticationAvailable === true
        ? "biometric_mechanism_available_constraint_not_selected"
        : "strong_biometric_unavailable",
      payload.biometricAuthenticationAvailable === true ? "unverified" : "unsupported",
    ),
    capabilityFact(
      "custody.authentication_validity_window",
      false,
      "authentication_window_verified",
      "one_shot_authorization_no_reuse_window",
      "unsupported",
    ),
    capabilityFact(
      "custody.enrollment_change_invalidation",
      false,
      "enrollment_change_invalidation_verified",
      "biometric_enrollment_constraint_not_selected",
      "unverified",
    ),
    capabilityFact(
      "custody.hardware_backed",
      secureEnclaveReady,
      "secure_enclave_private_key_operation_verified",
      "hardware_backed_operation_unverified",
      secureEnclaveReady ? "supported" : "unverified",
    ),
    capabilityFact(
      "custody.hardware_enforced_user_authentication",
      false,
      "hardware_user_authentication_verified",
      "hardware_user_authentication_not_verified",
      "unverified",
    ),
    capabilityFact("custody.android_keystore", false, "", "platform_not_applicable", "unsupported"),
    capabilityFact(
      "custody.apple_keychain",
      osStoreReady,
      "apple_keychain_operation_verified",
      "apple_keychain_operation_unavailable",
    ),
    capabilityFact("custody.linux_secret_service", false, "", "platform_not_applicable", "unsupported"),
    capabilityFact(
      "custody.data_protection_keychain",
      dataProtectionReady,
      "data_protection_keychain_operation_verified",
      "data_protection_keychain_operation_unavailable",
    ),
    capabilityFact("custody.tee", false, "", "platform_not_applicable", "unsupported"),
    capabilityFact("custody.strongbox", false, "", "platform_not_applicable", "unsupported"),
    capabilityFact(
      "custody.secure_enclave",
      secureEnclaveReady,
      "secure_enclave_private_key_operation_verified",
      "secure_enclave_operation_unavailable",
      secureEnclaveReady ? "supported" : "unsupported",
    ),
  ];
}

export function capabilityFact(capability, supported, supportedReason, unavailableReason, stateOverride) {
  const state = stateOverride || (supported ? "supported" : "unsupported");
  return {
    capability,
    state,
    reasonCode: supported ? supportedReason : unavailableReason,
  };
}

export function probeSucceeded(probe) {
  return probe?.itemCreated === true &&
    probe?.readMatched === true &&
    probe?.itemDeleted === true &&
    probe?.deviceOnlyAccessibilityObserved === true;
}

export function userPresenceOperationSucceeded(payload) {
  const proof = payload.userPresence || {};
  return proof.accessControlCreated === true &&
    proof.itemCreated === true &&
    proof.nonInteractiveReadBlocked === true &&
    proof.authorizedReadSucceeded === true &&
    proof.itemDeleted === true &&
    Number(payload.interactiveAuthorizationAttemptCount || 0) === 1 &&
    payload.interactiveAuthorizationSucceeded === true;
}

export function swiftSource() {
  return String.raw`
import Foundation
import LocalAuthentication
import Security

func randomData() -> Data {
  var bytes = [UInt8](repeating: 0, count: 32)
  _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
  return Data(bytes)
}

let service = "land.lico.licoup.secure-mesh.macos-adaptive-custody-proof"
let secret = randomData()

func baseQuery(account: String, dataProtection: Bool) -> [String: Any] {
  var query: [String: Any] = [
    kSecClass as String: kSecClassGenericPassword,
    kSecAttrService as String: service,
    kSecAttrAccount as String: account
  ]
  if dataProtection {
    query[kSecUseDataProtectionKeychain as String] = true
  }
  return query
}

func basicStoreProbe(dataProtection: Bool) -> [String: Any] {
  let account = "basic-\(UUID().uuidString)"
  var addQuery = baseQuery(account: account, dataProtection: dataProtection)
  addQuery[kSecValueData as String] = secret
  addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
  let addStatus = SecItemAdd(addQuery as CFDictionary, nil)

  var readQuery = baseQuery(account: account, dataProtection: dataProtection)
  readQuery[kSecReturnData as String] = true
  readQuery[kSecMatchLimit as String] = kSecMatchLimitOne
  readQuery[kSecUseAuthenticationUI as String] = kSecUseAuthenticationUIFail
  var copied: CFTypeRef?
  let readStatus = SecItemCopyMatching(readQuery as CFDictionary, &copied)
  let readMatched = readStatus == errSecSuccess && (copied as? Data) == secret

  var deleteQuery = baseQuery(account: account, dataProtection: dataProtection)
  deleteQuery[kSecUseAuthenticationUI as String] = kSecUseAuthenticationUIFail
  let deleteStatus = addStatus == errSecSuccess
    ? SecItemDelete(deleteQuery as CFDictionary)
    : errSecItemNotFound
  return [
    "itemCreated": addStatus == errSecSuccess,
    "readMatched": readMatched,
    "itemDeleted": addStatus != errSecSuccess || deleteStatus == errSecSuccess,
    "deviceOnlyAccessibilityObserved": addStatus == errSecSuccess && readMatched
  ]
}

struct PreparedUserPresence {
  let dataProtection: Bool
  let account: String
  let accessControlCreated: Bool
  let addStatus: OSStatus
  let nonInteractiveReadBlocked: Bool
}

func prepareUserPresence(dataProtection: Bool) -> PreparedUserPresence {
  let account = "presence-\(UUID().uuidString)"
  var accessError: Unmanaged<CFError>?
  let access = SecAccessControlCreateWithFlags(
    nil,
    kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
    .userPresence,
    &accessError
  )
  var addStatus: OSStatus = errSecParam
  if let access {
    var addQuery = baseQuery(account: account, dataProtection: dataProtection)
    addQuery[kSecValueData as String] = secret
    addQuery[kSecAttrAccessControl as String] = access
    addStatus = SecItemAdd(addQuery as CFDictionary, nil)
  }

  let nonInteractiveContext = LAContext()
  nonInteractiveContext.interactionNotAllowed = true
  var readQuery = baseQuery(account: account, dataProtection: dataProtection)
  readQuery[kSecReturnData as String] = true
  readQuery[kSecMatchLimit as String] = kSecMatchLimitOne
  readQuery[kSecUseAuthenticationUI as String] = kSecUseAuthenticationUIFail
  readQuery[kSecUseAuthenticationContext as String] = nonInteractiveContext
  var copied: CFTypeRef?
  let readStatus = addStatus == errSecSuccess
    ? SecItemCopyMatching(readQuery as CFDictionary, &copied)
    : errSecItemNotFound
  return PreparedUserPresence(
    dataProtection: dataProtection,
    account: account,
    accessControlCreated: access != nil,
    addStatus: addStatus,
    nonInteractiveReadBlocked:
      readStatus == errSecInteractionNotAllowed || readStatus == errSecAuthFailed
  )
}

func completeUserPresence(
  _ prepared: PreparedUserPresence,
  context: LAContext,
  authorized: Bool
) -> [String: Any] {
  var copied: CFTypeRef?
  var readStatus: OSStatus = errSecInteractionNotAllowed
  if authorized && prepared.addStatus == errSecSuccess {
    var readQuery = baseQuery(
      account: prepared.account,
      dataProtection: prepared.dataProtection
    )
    readQuery[kSecReturnData as String] = true
    readQuery[kSecMatchLimit as String] = kSecMatchLimitOne
    readQuery[kSecUseAuthenticationUI as String] = kSecUseAuthenticationUIFail
    readQuery[kSecUseAuthenticationContext as String] = context
    readStatus = SecItemCopyMatching(readQuery as CFDictionary, &copied)
  }
  let readMatched = readStatus == errSecSuccess && (copied as? Data) == secret

  let cleanupContext = LAContext()
  cleanupContext.interactionNotAllowed = true
  var deleteQuery = baseQuery(
    account: prepared.account,
    dataProtection: prepared.dataProtection
  )
  deleteQuery[kSecUseAuthenticationUI as String] = kSecUseAuthenticationUIFail
  deleteQuery[kSecUseAuthenticationContext as String] = authorized ? context : cleanupContext
  let deleteStatus = prepared.addStatus == errSecSuccess
    ? SecItemDelete(deleteQuery as CFDictionary)
    : errSecItemNotFound
  return [
    "selectedStore": prepared.dataProtection
      ? "data_protection_keychain"
      : "standard_keychain",
    "accessControlCreated": prepared.accessControlCreated,
    "itemCreated": prepared.addStatus == errSecSuccess,
    "nonInteractiveReadBlocked": prepared.nonInteractiveReadBlocked,
    "authorizedReadSucceeded": readMatched,
    "itemDeleted": prepared.addStatus != errSecSuccess || deleteStatus == errSecSuccess
  ]
}

func secureEnclaveOperationProbe() -> Bool {
  var accessError: Unmanaged<CFError>?
  guard let access = SecAccessControlCreateWithFlags(
    nil,
    kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
    .privateKeyUsage,
    &accessError
  ) else {
    return false
  }
  let attributes: [String: Any] = [
    kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
    kSecAttrKeySizeInBits as String: 256,
    kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave,
    kSecPrivateKeyAttrs as String: [
      kSecAttrIsPermanent as String: false,
      kSecAttrAccessControl as String: access
    ]
  ]
  var keyError: Unmanaged<CFError>?
  guard let privateKey = SecKeyCreateRandomKey(attributes as CFDictionary, &keyError),
        SecKeyCopyPublicKey(privateKey) != nil,
        SecKeyIsAlgorithmSupported(
          privateKey,
          .sign,
          .ecdsaSignatureMessageX962SHA256
        ) else {
    return false
  }
  var signatureError: Unmanaged<CFError>?
  return SecKeyCreateSignature(
    privateKey,
    .ecdsaSignatureMessageX962SHA256,
    randomData() as CFData,
    &signatureError
  ) != nil
}

let context = LAContext()
context.localizedReason = "Authorize LicoUp Secure Mesh local key access once."
var authError: NSError?
let localAuthenticationAvailable = context.canEvaluatePolicy(
  .deviceOwnerAuthentication,
  error: &authError
)
var biometricError: NSError?
let biometricAuthenticationAvailable = context.canEvaluatePolicy(
  .deviceOwnerAuthenticationWithBiometrics,
  error: &biometricError
)
let interactiveWorkflowSelected =
  ProcessInfo.processInfo.environment["LICO_MACOS_USER_PRESENCE_INTERACTIVE"] == "1"
let standardKeychain = basicStoreProbe(dataProtection: false)
let dataProtectionKeychain = basicStoreProbe(dataProtection: true)

var preparedUserPresence: PreparedUserPresence? = nil
if interactiveWorkflowSelected && localAuthenticationAvailable {
  if dataProtectionKeychain["itemCreated"] as? Bool == true {
    let candidate = prepareUserPresence(dataProtection: true)
    if candidate.addStatus == errSecSuccess {
      preparedUserPresence = candidate
    }
  }
  if preparedUserPresence == nil && standardKeychain["itemCreated"] as? Bool == true {
    let candidate = prepareUserPresence(dataProtection: false)
    if candidate.addStatus == errSecSuccess {
      preparedUserPresence = candidate
    }
  }
}

var interactiveAuthorizationAttemptCount = 0
var interactiveAuthorizationSucceeded = false
var interactiveAuthorizationTimedOut = false
if preparedUserPresence != nil {
  interactiveAuthorizationAttemptCount = 1
  let semaphore = DispatchSemaphore(value: 0)
  context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: context.localizedReason) { success, _ in
    interactiveAuthorizationSucceeded = success
    semaphore.signal()
  }
  interactiveAuthorizationTimedOut = semaphore.wait(timeout: .now() + 60) == .timedOut
  if interactiveAuthorizationTimedOut {
    context.invalidate()
  }
}

let skippedUserPresence: [String: Any] = [
  "selectedStore": "none",
  "accessControlCreated": false,
  "itemCreated": false,
  "nonInteractiveReadBlocked": false,
  "authorizedReadSucceeded": false,
  "itemDeleted": true
]
let userPresence = preparedUserPresence.map {
  completeUserPresence(
    $0,
    context: context,
    authorized: interactiveAuthorizationSucceeded && !interactiveAuthorizationTimedOut
  )
} ?? skippedUserPresence

let payload: [String: Any] = [
  "standardKeychain": standardKeychain,
  "dataProtectionKeychain": dataProtectionKeychain,
  "userPresence": userPresence,
  "localAuthenticationAvailable": localAuthenticationAvailable,
  "biometricAuthenticationAvailable": biometricAuthenticationAvailable,
  "secureEnclaveOperationSucceeded": secureEnclaveOperationProbe(),
  "singleAuthorizationContextCreated": true,
  "singleAuthorizationContextSharedByOperations": preparedUserPresence != nil,
  "interactiveWorkflowSelected": interactiveWorkflowSelected,
  "interactiveAuthorizationAttemptCount": interactiveAuthorizationAttemptCount,
  "interactiveAuthorizationSucceeded": interactiveAuthorizationSucceeded,
  "interactiveAuthorizationTimedOut": interactiveAuthorizationTimedOut,
  "automaticAuthorizationRetryUsed": false,
  "appPasswordPromptUsed": false,
  "appCredentialPromptUsed": false
]

let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
print(String(data: data, encoding: .utf8)!)
`;
}

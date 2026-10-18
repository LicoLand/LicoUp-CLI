export function artifactSecurityStateStable(before, after) {
  return before?.artifactDigest === after?.artifactDigest &&
    before?.signatureKind === after?.signatureKind &&
    before?.signatureVerified === true && after?.signatureVerified === true &&
    before?.hardenedRuntime === true && after?.hardenedRuntime === true &&
    before?.entitlementsMatch === true && after?.entitlementsMatch === true &&
    before?.entitlementsDigest === after?.entitlementsDigest &&
    before?.nestedCodeMinimalEntitlements === true &&
    after?.nestedCodeMinimalEntitlements === true;
}

export function artifactSecurityState(artifactDigest, signature, nestedReady) {
  return Object.freeze({
    artifactDigest,
    signatureKind: signature.signatureKind,
    signatureVerified: signature.verified === true,
    hardenedRuntime: signature.hardenedRuntime === true,
    entitlementsMatch: signature.entitlementsMatch === true,
    entitlementsDigest: signature.entitlementsDigest,
    nestedCodeMinimalEntitlements: nestedReady === true,
  });
}

export function nestedCodePolicyReady(policy) {
  return policy?.nestedSignatures?.length > 0 &&
    policy.nestedSignatures.every(({ signature: nestedSignature }) => {
    return nestedSignature.verified === true &&
      nestedSignature.signatureKind === "local-identity-codesign" &&
      nestedSignature.hardenedRuntime === true &&
      nestedSignature.entitlementsEmpty === true;
  });
}

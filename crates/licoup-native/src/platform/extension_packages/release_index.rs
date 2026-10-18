//! Read the signed release package index the release authority publishes.
//!
//! The index is one document per release target: one entry per independently
//! released package, each carrying the package's identity, version, client
//! compatibility, native converter declaration and payload digest. It is signed
//! by the same two Ed25519 release roles, over the same canonical unsigned bytes,
//! as the client update manifest — the release tool
//! (`tools/scripts/client-release-package-index.mjs`) owns producing it, and this
//! module owns reading it without a network and without an installed client.
//!
//! What this module decides is only what the document itself can decide:
//!
//! * the document is the index schema, within its published bounds, with unique
//!   package identities in order and two distinct signature roles;
//! * both role signatures verify over the canonical bytes, so a rewritten entry,
//!   a dropped role or a tampered digest is refused rather than read;
//! * one entry's converter declaration and payload digest are structurally
//!   usable, and payload bytes handed to [`verify_payload`] are the bytes that
//!   entry describes.
//!
//! Two facts are deliberately *not* decided here, because they are claims about
//! other owners' documents:
//!
//! * client compatibility decides whether a *host* may load a package, and it is
//!   read from the package's own manifest ([`PackageManifest::admit_client`]);
//!   the index copy is reported, not enforced, so a release index for another
//!   client line still describes the formats its packages own.
//! * which formats a package owns is the package's own declaration. The release
//!   declaration publishes one source format per package entry while the manifest
//!   declares the list it reads, so [`IndexedPackage::reconcile`] is the join:
//!   the entry must agree with the manifest, and the required source format must
//!   be one the manifest declares.

use super::refusal;
use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use licoup_application::ApplicationFailure;
use licoup_extension_contracts::manifest::{FrozenEndpoints, PackageManifest, is_converter_entry};
use licoup_extension_contracts::{is_namespaced, is_semver};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The index schema this reader accepts.
pub const PACKAGE_INDEX_SCHEMA: &str = "licomesh.client-release-package-index.v1";

/// The largest index document this reader accepts.
pub const MAX_INDEX_BYTES: usize = 1024 * 1024;

/// The most packages one index may describe.
pub const MAX_INDEX_PACKAGES: usize = 64;

/// The largest public key catalogue this reader accepts.
pub const MAX_PUBLIC_KEYS_BYTES: usize = 256 * 1024;

/// The stage every refusal from this reader names.
const INDEX_STAGE: &str = "extension/package-index";

/// The digest form the index publishes, and the form a payload is checked in.
pub const DIGEST_PREFIX: &str = "sha256:";

/// The release tracks an index may be published for.
const RELEASE_TRACKS: [&str; 2] = ["nightly", "stable"];

/// The key catalogue bundled with the client, for a caller that has no explicit
/// key document. It is the same catalogue the client update manifest verifies
/// against, so one release authority owns both documents.
pub fn bundled_public_keys() -> &'static str {
    include_str!("../../../resources/client-update-public-keys.json")
}

/// One converter as the release index publishes it.
///
/// It carries one source format rather than the manifest's list: a release
/// publishes the transition it is released for, and the manifest declares every
/// format the program reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedConverter {
    pub kind: String,
    pub entry: String,
    pub source_format: String,
    pub target_format: String,
}

/// One payload as the release index publishes it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedPayload {
    pub file_name: String,
    pub byte_size: u64,
    pub sha256: String,
}

/// One package entry that passed structural validation and signature verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedPackage {
    pub package_id: String,
    pub display_name: String,
    pub package_version: String,
    pub converter: IndexedConverter,
    pub payload: IndexedPayload,
}

impl IndexedPackage {
    /// Join one verified index entry with the package's own declaration.
    ///
    /// The entry is authenticated release metadata and the manifest is the
    /// package's own claim about its payload; a migration may act only where the
    /// two agree, and only for the pair the caller requires. Every disagreement is
    /// its own refusal so a caller can name what is wrong instead of reporting
    /// "not a converter".
    pub fn reconcile(
        &self,
        manifest: &PackageManifest,
        required: &FrozenEndpoints,
    ) -> Result<(), ApplicationFailure> {
        if self.package_id != manifest.id || self.package_version != manifest.version {
            return Err(index_refusal("package_index_identity_mismatch").with_field("packageId"));
        }
        // The required pair is asked of the manifest exactly as the conversion
        // contract answers it, so the two readers cannot disagree about which
        // package owns the conversion.
        let declaration = manifest.conversion_owner(required).map_err(|failure| {
            refusal(failure.code.as_str(), INDEX_STAGE).with_field("conversion")
        })?;
        if self.converter.kind != declaration.kind.as_str()
            || self.converter.entry != declaration.entry
            || self.converter.target_format != declaration.target_format
        {
            return Err(index_refusal("package_index_converter_mismatch").with_field("converter"));
        }
        // The index names the one source format this release publishes, and the
        // manifest names every format the converter reads. The index's format must
        // be the required one and must be one the manifest declares.
        if self.converter.source_format != required.source_format()
            || !declaration.converts_from(self.converter.source_format.as_str())
        {
            return Err(index_refusal("package_index_converter_mismatch")
                .with_field("converter.sourceFormat"));
        }
        Ok(())
    }

    /// Whether payload bytes are the bytes this entry describes.
    ///
    /// The size is checked before the digest so an oversized payload is refused
    /// without hashing it.
    pub fn verify_payload(&self, bytes: &[u8]) -> Result<(), ApplicationFailure> {
        if bytes.len() as u64 != self.payload.byte_size {
            return Err(index_refusal("package_index_payload_invalid")
                .with_presentation_arg("measuredBytes", &bytes.len().to_string()));
        }
        let digest = content_digest(bytes);
        if digest != self.payload.sha256 {
            return Err(index_refusal("package_index_payload_invalid")
                .with_presentation_arg("package", &self.package_id));
        }
        Ok(())
    }
}

/// One index whose signatures and structure verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedPackageIndex {
    pub release_track: String,
    pub packages: Vec<IndexedPackage>,
}

impl VerifiedPackageIndex {
    /// The entry for one package identity, when the index describes it.
    pub fn package(&self, package_id: &str) -> Option<&IndexedPackage> {
        self.packages
            .iter()
            .find(|entry| entry.package_id == package_id)
    }
}

/// The canonical bytes one index signature covers.
///
/// This is the same canonical form the release tool signs and the client update
/// verifier reads: keys sorted, no whitespace, and the `signatures` member
/// removed. It is published so a test or a verifier can recompute exactly what
/// the release authority signed instead of re-implementing the form.
pub fn canonical_unsigned_bytes(document: &Value) -> Vec<u8> {
    let mut unsigned = document.clone();
    if let Some(object) = unsigned.as_object_mut() {
        object.remove("signatures");
    }
    stable_stringify(&unsigned).into_bytes()
}

fn stable_stringify(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => if *flag { "true" } else { "false" }.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string()),
        Value::Array(items) => {
            let body = items
                .iter()
                .map(stable_stringify)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{body}]")
        }
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let body = keys
                .into_iter()
                .map(|key| {
                    let encoded = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
                    format!("{encoded}:{}", stable_stringify(&map[key]))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
    }
}

/// The digest of payload bytes, in the form the index publishes.
fn content_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let mut digest = String::from(DIGEST_PREFIX);
    for byte in hasher.finalize() {
        digest.push_str(&format!("{byte:02x}"));
    }
    digest
}

/// Verify one signed index against one public key catalogue.
///
/// Everything is decided before anything is returned: the document shape, the
/// entry shapes, the two role signatures and the role policy. A caller that
/// receives a [`VerifiedPackageIndex`] holds entries whose published values were
/// covered by both release signatures.
pub fn verify_index(
    index_text: &str,
    public_keys_text: &str,
) -> Result<VerifiedPackageIndex, ApplicationFailure> {
    if index_text.len() > MAX_INDEX_BYTES || public_keys_text.len() > MAX_PUBLIC_KEYS_BYTES {
        return Err(index_refusal("package_index_invalid"));
    }
    let document: Value =
        serde_json::from_str(index_text).map_err(|_| index_refusal("package_index_invalid"))?;
    let root = document
        .as_object()
        .ok_or_else(|| index_refusal("package_index_invalid"))?;
    if !exact_keys(
        root,
        &[
            "schemaVersion",
            "releaseTrack",
            "packages",
            "signaturePolicy",
            "signatures",
        ],
    ) {
        return Err(index_refusal("package_index_invalid"));
    }
    if text(root.get("schemaVersion")) != PACKAGE_INDEX_SCHEMA {
        return Err(index_refusal("package_index_invalid"));
    }
    let release_track = text(root.get("releaseTrack"));
    if !RELEASE_TRACKS.contains(&release_track.as_str()) {
        return Err(index_refusal("package_index_invalid"));
    }
    let packages = root
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| index_refusal("package_index_invalid"))?;
    if packages.is_empty() || packages.len() > MAX_INDEX_PACKAGES {
        return Err(index_refusal("package_index_invalid"));
    }
    let mut entries = Vec::with_capacity(packages.len());
    let mut seen = BTreeSet::new();
    let mut previous: Option<String> = None;
    for package in packages {
        let entry = read_entry(package)?;
        if !seen.insert(entry.package_id.clone()) {
            return Err(index_refusal("package_index_package_order_invalid"));
        }
        if previous
            .as_deref()
            .is_some_and(|previous| previous >= entry.package_id.as_str())
        {
            return Err(index_refusal("package_index_package_order_invalid"));
        }
        previous = Some(entry.package_id.clone());
        entries.push(entry);
    }
    verify_signatures(&document, root, public_keys_text)?;
    Ok(VerifiedPackageIndex {
        release_track,
        packages: entries,
    })
}

/// Verify both declared release roles signed this document.
fn verify_signatures(
    document: &Value,
    root: &serde_json::Map<String, Value>,
    public_keys_text: &str,
) -> Result<(), ApplicationFailure> {
    let policy = root
        .get("signaturePolicy")
        .and_then(Value::as_object)
        .ok_or_else(|| index_refusal("package_index_invalid"))?;
    if !exact_keys(policy, &["offlineRootKeyId", "onlineSigningKeyId"]) {
        return Err(index_refusal("package_index_invalid"));
    }
    let offline_root_key_id = text(policy.get("offlineRootKeyId"));
    let online_signing_key_id = text(policy.get("onlineSigningKeyId"));
    if namespaced_or_empty(&offline_root_key_id)
        || namespaced_or_empty(&online_signing_key_id)
        || offline_root_key_id == online_signing_key_id
    {
        return Err(index_refusal("package_index_key_policy_invalid"));
    }
    let keys = read_public_keys(public_keys_text)?;
    let signatures = root
        .get("signatures")
        .and_then(Value::as_array)
        .ok_or_else(|| index_refusal("package_index_signature_invalid"))?;
    if signatures.is_empty() {
        return Err(index_refusal("package_index_signature_invalid"));
    }
    let payload = canonical_unsigned_bytes(document);
    let mut verified = BTreeSet::new();
    for entry in signatures {
        let Some(object) = entry.as_object() else {
            return Err(index_refusal("package_index_signature_invalid"));
        };
        if !exact_keys(object, &["keyId", "algorithm", "signature"]) {
            return Err(index_refusal("package_index_signature_invalid"));
        }
        let key_id = text(object.get("keyId"));
        if text(object.get("algorithm")) != "Ed25519"
            || !keys.contains_key(&key_id)
            || verified.contains(&key_id)
        {
            return Err(index_refusal("package_index_signature_invalid"));
        }
        let encoded = text(object.get("signature"));
        let raw = general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|_| index_refusal("package_index_signature_invalid"))?;
        let signature = Signature::from_slice(&raw)
            .map_err(|_| index_refusal("package_index_signature_invalid"))?;
        keys[&key_id]
            .verify(&payload, &signature)
            .map_err(|_| index_refusal("package_index_signature_invalid"))?;
        verified.insert(key_id);
    }
    if !verified.contains(&offline_root_key_id) || !verified.contains(&online_signing_key_id) {
        return Err(index_refusal("package_index_signature_roles_incomplete"));
    }
    Ok(())
}

fn read_public_keys(
    public_keys_text: &str,
) -> Result<BTreeMap<String, VerifyingKey>, ApplicationFailure> {
    let document: Value = serde_json::from_str(public_keys_text)
        .map_err(|_| index_refusal("package_index_public_keys_invalid"))?;
    let keys = document
        .get("keys")
        .and_then(Value::as_object)
        .ok_or_else(|| index_refusal("package_index_public_keys_invalid"))?;
    if keys.is_empty() {
        return Err(index_refusal("package_index_public_keys_invalid"));
    }
    let mut decoded = BTreeMap::new();
    for (key_id, entry) in keys {
        if !is_namespaced(key_id) {
            return Err(index_refusal("package_index_public_keys_invalid"));
        }
        let encoded = match entry {
            Value::String(value) => value.clone(),
            Value::Object(object) if exact_keys(object, &["publicKey"]) => {
                text(object.get("publicKey"))
            }
            _ => return Err(index_refusal("package_index_public_keys_invalid")),
        };
        let raw = general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|_| index_refusal("package_index_public_keys_invalid"))?;
        let array: [u8; 32] = raw
            .as_slice()
            .try_into()
            .map_err(|_| index_refusal("package_index_public_keys_invalid"))?;
        let key = VerifyingKey::from_bytes(&array)
            .map_err(|_| index_refusal("package_index_public_keys_invalid"))?;
        decoded.insert(key_id.clone(), key);
    }
    Ok(decoded)
}

/// Read one package entry of an index document.
fn read_entry(value: &Value) -> Result<IndexedPackage, ApplicationFailure> {
    let Some(object) = value.as_object() else {
        return Err(index_refusal("package_index_package_invalid"));
    };
    if !exact_keys(
        object,
        &[
            "packageId",
            "displayName",
            "packageVersion",
            "hostProtocol",
            "clientCompatibility",
            "converter",
            "payload",
        ],
    ) {
        return Err(index_refusal("package_index_package_invalid"));
    }
    let package_id = text(object.get("packageId"));
    let display_name = text(object.get("displayName"));
    let package_version = text(object.get("packageVersion"));
    if !is_namespaced(&package_id)
        || display_name.is_empty()
        || !is_semver(&package_version)
        || !object
            .get("hostProtocol")
            .and_then(Value::as_object)
            .is_some_and(|protocol| exact_keys(protocol, &["major", "minimumMinor"]))
        || object
            .get("clientCompatibility")
            .and_then(Value::as_object)
            .is_none()
    {
        return Err(index_refusal("package_index_package_invalid"));
    }
    let converter = object
        .get("converter")
        .and_then(Value::as_object)
        .ok_or_else(|| index_refusal("package_index_package_invalid"))?;
    if !exact_keys(
        converter,
        &["kind", "entry", "sourceFormat", "targetFormat"],
    ) {
        return Err(index_refusal("package_index_package_invalid"));
    }
    let entry = text(converter.get("entry"));
    if text(converter.get("kind")) != "native-executable" || !is_converter_entry(&entry) {
        return Err(index_refusal("package_index_package_invalid"));
    }
    let source_format = text(converter.get("sourceFormat"));
    let target_format = text(converter.get("targetFormat"));
    if source_format.is_empty() || target_format.is_empty() || source_format == target_format {
        return Err(index_refusal("package_index_package_invalid"));
    }
    let payload = object
        .get("payload")
        .and_then(Value::as_object)
        .ok_or_else(|| index_refusal("package_index_package_invalid"))?;
    if !exact_keys(payload, &["fileName", "byteSize", "sha256"]) {
        return Err(index_refusal("package_index_package_invalid"));
    }
    let file_name = text(payload.get("fileName"));
    let byte_size = payload
        .get("byteSize")
        .and_then(Value::as_u64)
        .ok_or_else(|| index_refusal("package_index_package_invalid"))?;
    let sha256 = text(payload.get("sha256"));
    if file_name.is_empty()
        || file_name != file_name.rsplit(['/', '\\']).next().unwrap_or_default()
        || byte_size == 0
        || !valid_digest(&sha256)
    {
        return Err(index_refusal("package_index_package_invalid"));
    }
    Ok(IndexedPackage {
        package_id,
        display_name,
        package_version,
        converter: IndexedConverter {
            kind: text(converter.get("kind")),
            entry,
            source_format,
            target_format,
        },
        payload: IndexedPayload {
            file_name,
            byte_size,
            sha256,
        },
    })
}

fn exact_keys(object: &serde_json::Map<String, Value>, keys: &[&str]) -> bool {
    object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
}

fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

fn valid_digest(value: &str) -> bool {
    value.len() == DIGEST_PREFIX.len() + 64
        && value.starts_with(DIGEST_PREFIX)
        && value[DIGEST_PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn namespaced_or_empty(value: &str) -> bool {
    value.is_empty() || !is_namespaced(value)
}

/// A refusal from this reader, on the component the package store publishes.
fn index_refusal(code: &str) -> ApplicationFailure {
    refusal(code, INDEX_STAGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    const OFFLINE: &str = "test-offline-root";
    const ONLINE: &str = "test-online-signing";

    fn key_pair() -> (SigningKey, String) {
        // Any valid pair proves the rule; nothing here depends on a fixed key.
        let mut bytes = [0_u8; 32];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
        let key = SigningKey::from_bytes(&bytes);
        let public = general_purpose::STANDARD
            .encode(key.verifying_key().to_bytes())
            .to_string();
        (key, public)
    }

    fn public_keys_text(offline: &str, online: &str) -> String {
        json!({
            "keys": {
                OFFLINE: { "publicKey": offline },
                ONLINE: { "publicKey": online },
            }
        })
        .to_string()
    }

    fn index_document() -> Value {
        json!({
            "schemaVersion": PACKAGE_INDEX_SCHEMA,
            "releaseTrack": "stable",
            "packages": [{
                "packageId": "org.licoland.fixture.native-converter",
                "displayName": "Fixture converter",
                "packageVersion": "1.3.0",
                "hostProtocol": { "major": 1, "minimumMinor": 0 },
                "clientCompatibility": { "kind": "range", "range": ">=0.2.0, <1.0.0" },
                "converter": {
                    "kind": "native-executable",
                    "entry": "bin/converter",
                    "sourceFormat": "licoup-state-0.1.1",
                    "targetFormat": "licoup-state-0.3.0",
                },
                "payload": {
                    "fileName": "LicoUp-package-fixture.licopkg",
                    "byteSize": 4,
                    "sha256": content_digest(b"pkgs"),
                },
            }],
            "signaturePolicy": {
                "offlineRootKeyId": OFFLINE,
                "onlineSigningKeyId": ONLINE,
            },
        })
    }

    fn sign(document: &Value, offline: &SigningKey, online: &SigningKey) -> String {
        let payload = canonical_unsigned_bytes(document);
        let mut signed = document.clone();
        signed["signatures"] = json!([
            {
                "keyId": OFFLINE,
                "algorithm": "Ed25519",
                "signature": general_purpose::STANDARD.encode(offline.sign(&payload).to_bytes()),
            },
            {
                "keyId": ONLINE,
                "algorithm": "Ed25519",
                "signature": general_purpose::STANDARD.encode(online.sign(&payload).to_bytes()),
            },
        ]);
        signed.to_string()
    }

    fn verified() -> (VerifiedPackageIndex, String) {
        let (offline, offline_public) = key_pair();
        let (online, online_public) = key_pair();
        let keys = public_keys_text(&offline_public, &online_public);
        let index = verify_index(&sign(&index_document(), &offline, &online), &keys)
            .expect("the signed index verifies");
        (index, keys)
    }

    #[test]
    fn both_release_roles_sign_the_entry_that_is_read() {
        let (index, _keys) = verified();
        assert_eq!(index.release_track, "stable");
        let entry = index
            .package("org.licoland.fixture.native-converter")
            .expect("the entry is present");
        assert_eq!(entry.package_version, "1.3.0");
        assert_eq!(entry.converter.entry, "bin/converter");
        assert_eq!(entry.converter.source_format, "licoup-state-0.1.1");
        assert_eq!(entry.payload.byte_size, 4);
        entry.verify_payload(b"pkgs").expect("the payload matches");
    }

    #[test]
    fn a_rewritten_entry_or_a_missing_role_is_refused() {
        let (offline, offline_public) = key_pair();
        let (online, online_public) = key_pair();
        let keys = public_keys_text(&offline_public, &online_public);
        let signed = sign(&index_document(), &offline, &online);

        let mut rewritten: Value = serde_json::from_str(&signed).expect("index json");
        rewritten["packages"][0]["converter"]["sourceFormat"] = json!("licoup-state-9.9.9");
        assert_eq!(
            verify_index(&rewritten.to_string(), &keys)
                .expect_err("a rewritten entry is not signed")
                .code,
            "package_index_signature_invalid"
        );

        let mut single: Value = serde_json::from_str(&signed).expect("index json");
        single["signatures"] = json!([single["signatures"][0].clone()]);
        assert_eq!(
            verify_index(&single.to_string(), &keys)
                .expect_err("both roles are required")
                .code,
            "package_index_signature_roles_incomplete"
        );

        let mut extended: Value = serde_json::from_str(&signed).expect("index json");
        extended["eligibilityOverride"] = json!(true);
        assert_eq!(
            verify_index(&extended.to_string(), &keys)
                .expect_err("an unsupported member is refused")
                .code,
            "package_index_invalid"
        );
    }

    #[test]
    fn the_payload_identity_is_the_one_the_entry_publishes() {
        let (index, _keys) = verified();
        let entry = index
            .package("org.licoland.fixture.native-converter")
            .expect("entry");
        assert_eq!(
            entry.verify_payload(b"pkg!").expect_err("digest").code,
            "package_index_payload_invalid"
        );
        assert_eq!(
            entry.verify_payload(b"pkgs-extra").expect_err("size").code,
            "package_index_payload_invalid"
        );
    }

    #[test]
    fn a_bundled_key_catalogue_is_readable() {
        let document: Value =
            serde_json::from_str(bundled_public_keys()).expect("the bundled catalogue is JSON");
        let keys = read_public_keys(&document.to_string()).expect("the catalogue decodes");
        assert!(keys.len() >= 2, "two release roles are published");
    }

    fn manifest_conversion(entry: &str, sources: &[&str], target: &str) -> serde_json::Value {
        json!({
            "schema": "licoup.extension-package.v1",
            "id": "org.licoland.fixture.native-converter",
            "version": "1.3.0",
            "displayName": "Fixture converter",
            "hostProtocol": { "major": 1, "minimumMinor": 0 },
            "compatibility": { "clientVersions": [">=0.2.0, <1.0.0"] },
            "profiles": [{ "id": "agent-execution", "major": 1 }],
            "runtime": { "mode": "process", "entry": entry },
            "conversion": {
                "kind": "native-executable",
                "entry": entry,
                "sourceFormats": sources,
                "targetFormat": target,
            },
            "activation": "on-demand",
            "requires": [],
            "optionalRequires": [],
            "permissions": [],
            "contributions": [],
        })
    }

    fn required() -> FrozenEndpoints {
        FrozenEndpoints::new("licoup-state-0.1.1", "licoup-state-0.3.0")
    }

    #[test]
    fn the_entry_joins_the_manifest_that_declares_the_required_pair() {
        let (index, _keys) = verified();
        let entry = index
            .package("org.licoland.fixture.native-converter")
            .expect("entry");
        let manifest = PackageManifest::from_value(manifest_conversion(
            "bin/converter",
            &["licoup-state-0.1.0", "licoup-state-0.1.1"],
            "licoup-state-0.3.0",
        ))
        .expect("manifest");
        entry
            .reconcile(&manifest, &required())
            .expect("the release metadata and the package declaration agree");

        // The index's one source format must be a format the manifest declares.
        let narrowed = PackageManifest::from_value(manifest_conversion(
            "bin/converter",
            &["licoup-state-0.1.0"],
            "licoup-state-0.3.0",
        ))
        .expect("manifest");
        assert_eq!(
            entry
                .reconcile(&narrowed, &required())
                .expect_err("the manifest does not read the published source")
                .code,
            "manifest_conversion_endpoint_mismatch"
        );

        // And a different entry or target is a disagreement between two published documents.
        let other_entry = PackageManifest::from_value(manifest_conversion(
            "bin/other-converter",
            &["licoup-state-0.1.1"],
            "licoup-state-0.3.0",
        ))
        .expect("manifest");
        assert_eq!(
            entry
                .reconcile(&other_entry, &required())
                .expect_err("the entries disagree")
                .code,
            "package_index_converter_mismatch"
        );

        // A manifest that declares another pair is refused by the manifest's own
        // rule first: this package does not own the required conversion at all.
        let other_target = PackageManifest::from_value(manifest_conversion(
            "bin/converter",
            &["licoup-state-0.1.1"],
            "licoup-state-0.4.0",
        ))
        .expect("manifest");
        assert_eq!(
            entry
                .reconcile(
                    &other_target,
                    &FrozenEndpoints::new("licoup-state-0.1.1", "licoup-state-0.3.0")
                )
                .expect_err("the manifest does not declare the required pair")
                .code,
            "manifest_conversion_endpoint_mismatch"
        );

        // And when the manifest does own the pair, an index entry that names another
        // target is the disagreement between the two published documents.
        let good = PackageManifest::from_value(manifest_conversion(
            "bin/converter",
            &["licoup-state-0.1.1"],
            "licoup-state-0.3.0",
        ))
        .expect("manifest");
        let mut mismatched = entry.clone();
        mismatched.converter.target_format = "licoup-state-0.4.0".to_string();
        assert_eq!(
            mismatched
                .reconcile(&good, &required())
                .expect_err("the published targets disagree")
                .code,
            "package_index_converter_mismatch"
        );
        let mut rewrote_source = entry.clone();
        rewrote_source.converter.source_format = "licoup-state-0.1.0".to_string();
        assert_eq!(
            rewrote_source
                .reconcile(&good, &required())
                .expect_err("the published source is not the required one")
                .code,
            "package_index_converter_mismatch"
        );
    }

    #[test]
    fn an_index_for_another_identity_is_not_this_packages_entry() {
        let (index, _keys) = verified();
        let entry = index
            .package("org.licoland.fixture.native-converter")
            .expect("entry");
        let mut document = manifest_conversion(
            "bin/converter",
            &["licoup-state-0.1.1"],
            "licoup-state-0.3.0",
        );
        document["version"] = json!("1.4.0");
        let manifest = PackageManifest::from_value(document).expect("manifest");
        assert_eq!(
            entry
                .reconcile(&manifest, &required())
                .expect_err("the version disagrees")
                .code,
            "package_index_identity_mismatch"
        );
    }
}

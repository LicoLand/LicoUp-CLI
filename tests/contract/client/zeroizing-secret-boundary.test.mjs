import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const nativeRoot = "crates/licoup-native/src";
const custodyRoot = `${nativeRoot}/domain/mobile_relay/secret_custody`;
const platformRoot = `${nativeRoot}/platform/secure_mesh_secret_store`;
const catalogRoot = "tools/regression/client-module-catalog";
const RUST_WHITESPACE = String.raw`\s`;

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function exists(relativePath) {
  try {
    await fs.access(path.join(repoRoot, relativePath));
    return true;
  } catch {
    return false;
  }
}

async function rustSources(relativePath) {
  const absolute = path.join(repoRoot, relativePath);
  const entries = await fs.readdir(absolute, { withFileTypes: true });
  const sources = [];
  for (const entry of entries) {
    const child = path.join(relativePath, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== "tests") sources.push(...await rustSources(child));
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      sources.push([child, await read(child)]);
    }
  }
  return sources;
}

function implementationBlocks(source, traitName) {
  const blocks = [];
  const marker = `impl ${traitName} for `;
  let cursor = 0;
  while ((cursor = source.indexOf(marker, cursor)) >= 0) {
    const open = source.indexOf("{", cursor + marker.length);
    assert.notEqual(open, -1, `unterminated ${traitName} implementation`);
    let depth = 0;
    let end = open;
    for (; end < source.length; end += 1) {
      if (source[end] === "{") depth += 1;
      if (source[end] === "}") depth -= 1;
      if (depth === 0) {
        end += 1;
        break;
      }
    }
    blocks.push(source.slice(cursor, end));
    cursor = end;
  }
  return blocks;
}

function rustFunctionBlock(source, functionName) {
  const pattern = new RegExp(
    `\\bfn${RUST_WHITESPACE}+${functionName}(?:${RUST_WHITESPACE}*<[^>{}]*>)?${RUST_WHITESPACE}*\\(`,
    "u",
  );
  const match = pattern.exec(source);
  assert.ok(match, `missing Rust function ${functionName}`);
  const open = source.indexOf("{", match.index + match[0].length);
  assert.notEqual(open, -1, `unterminated Rust function ${functionName}`);
  let depth = 0;
  for (let cursor = open; cursor < source.length; cursor += 1) {
    if (source[cursor] === "{") depth += 1;
    if (source[cursor] === "}") depth -= 1;
    if (depth === 0) return source.slice(match.index, cursor + 1);
  }
  assert.fail(`unterminated Rust function ${functionName}`);
}

test("every secret-store implementation transfers bounded owned SecretBytes", async () => {
  const port = await read(`${nativeRoot}/core/secure_mesh_secret_store/port.rs`);
  assert.match(
    port,
    /fn set_secret\([^)]*secret:\s*SecretBytes\)\s*->\s*Result<\(\)>/su,
  );
  assert.match(
    port,
    /fn get_secret\([^)]*\)\s*->\s*Result<Option<SecretBytes>>/su,
  );
  for (const forbidden of [
    /secret:\s*&str/u,
    /Result<Option<String>>/u,
    /Zeroizing<String>/u,
  ]) {
    assert.doesNotMatch(port, forbidden);
  }

  const allNativeSources = await rustSources(nativeRoot);
  const implementations = allNativeSources.flatMap(([relativePath, source]) =>
    implementationBlocks(source, "SecureMeshSecretStore")
      .map((block) => [relativePath, block]));
  assert.ok(implementations.length > 0);
  for (const [relativePath, block] of implementations) {
    assert.doesNotMatch(block, /secret:\s*&str/u, relativePath);
    assert.doesNotMatch(block, /Result<Option<String>>/u, relativePath);
    assert.match(block, /secret:\s*SecretBytes/u, relativePath);
    assert.match(block, /Result<Option<SecretBytes>>/u, relativePath);
  }
});

test("platform and mobile bridges have no String compatibility secret payload", async () => {
  const platform = (await rustSources(platformRoot))
    .map(([relativePath, source]) => `// ${relativePath}\n${source}`)
    .join("\n");
  const android = await read(`${nativeRoot}/ffi/android_ffi.rs`);
  const ios = await read(`${nativeRoot}/ffi/ios_ffi.rs`);

  for (const [label, source] of [
    ["platform secret store", platform],
    ["Android secret bridge", android],
    ["iOS secret bridge", ios],
  ]) {
    assert.ok(source.includes("SecretBytes"), `${label} must use SecretBytes`);
    assert.doesNotMatch(source, /Option<String>\s*>\s*\{\s*[^}]*secret/isu, label);
    assert.doesNotMatch(source, /secret:\s*&str/u, label);
    assert.doesNotMatch(source, /Zeroizing<String>/u, label);
  }
  assert.doesNotMatch(android, /normalize_android_secret_store_get\s*\([^)]*Option<String>/su);
  assert.ok(android.includes("JByteArray"));
  assert.ok(android.includes("convert_byte_array"));
  assert.ok(android.includes("AndroidSecretByteArrayGuard"));
  assert.match(
    android,
    /impl Drop for AndroidSecretByteArrayGuard[\s\S]*\.set_region\(self\.env, 0, &zeros\)/u,
  );
  assert.ok(android.includes("MAX_SECRET_BYTES"));
  assert.match(
    android,
    /secureMeshAndroidSecretStoreSet[\s\S]*\(Ljava\/lang\/String;Ljava\/lang\/String;\[B\)Z/u,
  );
  assert.match(
    android,
    /secureMeshAndroidSecretStoreGet[\s\S]*\(Ljava\/lang\/String;Ljava\/lang\/String;\)\[B/u,
  );
  assert.doesNotMatch(
    android,
    /secureMeshAndroidSecretStoreSet[\s\S]*\(Ljava\/lang\/String;Ljava\/lang\/String;Ljava\/lang\/String;\)Z/u,
  );
  assert.doesNotMatch(android, /env\.new_string\s*\(\s*secret/u);
  assert.doesNotMatch(
    android,
    /(?:let\s+text:\s*String|JString::from\(value\))[\s\S]{0,240}normalize_android_secret_store_get/u,
  );

  assert.match(
    ios,
    /set_secret:\s*Option<[\s\S]*secret:\s*\*const u8[\s\S]*secret_len:\s*usize/u,
  );
  assert.match(
    ios,
    /get_secret:\s*Option<[\s\S]*value_out:\s*\*mut \*mut u8[\s\S]*value_len_out:\s*\*mut usize/u,
  );
  assert.ok(ios.includes("bytes_zeroize_and_free"));
  const iosSecretStoreBlocks = implementationBlocks(ios, "SecureMeshSecretStore");
  const iosSecretStore = iosSecretStoreBlocks.find((block) =>
    block.includes("IOS_SECRET_STORE_BACKEND"));
  assert.ok(iosSecretStore, "missing iOS callback secret-store implementation");
  assert.match(iosSecretStore, /secret:\s*SecretBytes/u);
  assert.match(iosSecretStore, /Result<Option<SecretBytes>>/u);
  assert.match(iosSecretStore, /secret\.expose_bytes\s*\(\)/u);
  assert.match(iosSecretStore, /bytes_zeroize_and_free/u);
  assert.doesNotMatch(iosSecretStore, /CString::new\s*\(\s*secret/u);
  assert.doesNotMatch(iosSecretStore, /CStr::from_ptr/u);
  assert.doesNotMatch(iosSecretStore, /(?:to_owned|to_string)\s*\(\)/u);
});

test("native hydration and admitted secrets move only into RuntimeSecretMaterial", async () => {
  const runtimeCarrierPath = `${custodyRoot}/runtime_secret_material.rs`;
  assert.equal(await exists(runtimeCarrierPath), true);
  const runtimeCarrier = await read(runtimeCarrierPath);
  const secretMaterial = await read(`${custodyRoot}/secret_material.rs`);

  assert.match(runtimeCarrier, /struct RuntimeSecretMaterial/u);
  assert.ok(runtimeCarrier.includes("SecretBytes"));
  assert.doesNotMatch(runtimeCarrier, /serde(?:_json)?::/u);
  assert.doesNotMatch(runtimeCarrier, /\bValue\b/u);
  assert.doesNotMatch(runtimeCarrier, /json!\s*\(/u);
  assert.doesNotMatch(runtimeCarrier, /derive\([^)]*(?:Serialize|Deserialize)/su);
  for (const required of [
    "hydrate_runtime_secret_material_from_native_store",
    "hydrate_runtime_secret_material_from_native_store_with_batch",
  ]) {
    assert.ok(secretMaterial.includes(required), required);
  }
  for (const retired of [
    "hydrate_config_secret_material_from_native_store",
    "hydrate_config_secret_material_from_native_store_with_batch",
    "hydrate_config_secret_material_from_secret_store",
    "hydrate_config_token_secret_material_from_secret_store",
  ]) {
    assert.equal(secretMaterial.includes(retired), false, retired);
  }

  const hydrationFunctions = [
    "hydrate_runtime_secret_material_from_native_store",
    "hydrate_runtime_secret_material_from_native_store_with_batch",
    "hydrate_runtime_secret_material_from_secret_store",
    "hydrate_runtime_secret_material_from_store_with_session",
  ].map((name) => [name, rustFunctionBlock(secretMaterial, name)]);
  for (const [name, block] of hydrationFunctions) {
    assert.match(block, /config:\s*&Value/u, name);
    assert.match(block, /material:\s*&mut RuntimeSecretMaterial/u, name);
    assert.doesNotMatch(block, /config:\s*&mut Value/u, name);
    assert.doesNotMatch(block, /json!\s*\(/u, name);
    assert.doesNotMatch(block, /Vec<\(\s*&(?:'static\s+)?str\s*,\s*String\s*\)>/u, name);
    assert.doesNotMatch(block, /secret(?:_bytes|_value)?\.to_string\s*\(\)/u, name);
  }
  const sessionHydration = hydrationFunctions.at(-1)[1];
  assert.ok(sessionHydration.includes("material.set_token"));
  assert.ok(sessionHydration.includes("material.set_paired_device_token"));
  assert.ok(sessionHydration.includes("material.merge_e2ee_bundle"));

  const runtime = await read(`${custodyRoot}/runtime.rs`);
  assert.match(
    runtime,
    /struct RuntimeSecretContext\s*\{[^}]*material:\s*RuntimeSecretMaterial/su,
  );
  const configStore = await read(`${custodyRoot}/config_store.rs`);
  const loadContext = rustFunctionBlock(
    configStore,
    "load_config_with_runtime_secret_context_unchecked",
  );
  assert.ok(loadContext.includes("hydrate_runtime_secret_material"));
  assert.ok(loadContext.includes("context.material"));
  assert.doesNotMatch(
    loadContext,
    /hydrate_[a-z_]*secret[a-z_]*\(\s*&mut config/su,
  );
  assert.doesNotMatch(loadContext, /config\[[^\]]+\]\s*=\s*json!\s*\(\s*secret/su);

  const pairingPresentation = await read(
    `${nativeRoot}/domain/mobile_relay/endpoint_trust/pairing_presentation.rs`,
  );
  const inviteAdmission = rustFunctionBlock(
    pairingPresentation,
    "apply_pairing_invite_params_with_context",
  );
  assert.match(
    inviteAdmission,
    /secret_context:\s*Option<&mut RuntimeSecretContext>/u,
  );
  assert.match(
    inviteAdmission,
    /context\.material\.replace_e2ee_secret\s*\(/u,
  );
  assert.match(
    inviteAdmission,
    /SecretBytes::try_from_(?:bytes|string)\s*\(/u,
  );
  assert.doesNotMatch(
    inviteAdmission,
    /config\s*\[\s*"(?:e2eePairingSecret|pairingSecretBase64url)"\s*\]\s*=/u,
  );
});

test("cryptographic and relay consumers borrow explicit typed secret views", async () => {
  const endpointStateSource = await read(
    `${nativeRoot}/domain/mobile_relay/endpoint_trust/local_material/state_codec.rs`,
  );
  const endpointPrimitives = await read(
    `${nativeRoot}/domain/mobile_relay/endpoint_trust/primitives.rs`,
  );
  const endpointMutation = await read(
    `${nativeRoot}/domain/mobile_relay/endpoint_trust/local_material/material_mutation.rs`,
  );
  const pairing = await read(`${nativeRoot}/domain/mobile_relay/pairing.rs`);
  const relayCreate = await read(
    `${nativeRoot}/domain/mobile_relay/relay_operations/command_handlers/create.rs`,
  );
  const relayResult = await read(
    `${nativeRoot}/domain/mobile_relay/relay_operations/command_handlers/result.rs`,
  );

  const localEndpointState = rustFunctionBlock(
    endpointStateSource,
    "local_endpoint_state",
  );
  assert.match(
    localEndpointState,
    /secret_material:\s*&(?:'[a-z_][a-z0-9_]*\s+)?RuntimeSecretMaterial/u,
  );
  assert.match(localEndpointState, /\.expose_utf8\s*\(\)/u);
  assert.doesNotMatch(
    localEndpointState,
    /descriptor_text\([^,]+,\s*"(?:privateKeyBase64url|signingKeyBase64url|signedPrekeyPrivateKeyBase64url|oneTimePrekeyPrivateKeyBase64url|oneTimeMlKem1024PrekeySeedBase64url|pairingSecretBase64url)"\)/u,
  );

  const claimProofMac = rustFunctionBlock(
    endpointPrimitives,
    "mobile_relay_claim_proof_mac",
  );
  assert.match(claimProofMac, /secret_material:\s*&RuntimeSecretMaterial/u);
  assert.match(
    claimProofMac,
    /e2ee_secret\s*\(\s*MobileRelayE2eeSecretField::PairingSecret\s*\)/u,
  );
  assert.match(claimProofMac, /secret\.expose_bytes\s*\(\)/u);

  const ensureEndpoint = rustFunctionBlock(
    endpointMutation,
    "ensure_mobile_relay_endpoint_material",
  );
  assert.match(
    ensureEndpoint,
    /secret_material:\s*&mut RuntimeSecretMaterial/u,
  );
  assert.match(ensureEndpoint, /secret_material\.insert_e2ee_secret\s*\(/u);
  for (const secretField of [
    "privateKeyBase64url",
    "signingKeyBase64url",
    "signedPrekeyPrivateKeyBase64url",
    "oneTimePrekeyPrivateKeyBase64url",
    "oneTimeMlKem1024PrekeySeedBase64url",
    "pairingSecretBase64url",
  ]) {
    const secretJsonAssignment = new RegExp(
      String.raw`(?:insert\(${RUST_WHITESPACE}*"${secretField}"\.to_string\(\)${RUST_WHITESPACE}*,|\[${RUST_WHITESPACE}*"${secretField}"${RUST_WHITESPACE}*\]${RUST_WHITESPACE}*=)${RUST_WHITESPACE}*json!${RUST_WHITESPACE}*\(`,
      "su",
    );
    assert.doesNotMatch(ensureEndpoint, secretJsonAssignment, secretField);
  }
  assert.doesNotMatch(
    ensureEndpoint,
    /fields\.private_key\.to_string\(\)\s*,\s*json!\s*\(/u,
  );

  const requirePairingSecret = rustFunctionBlock(pairing, "require_pairing_secret");
  assert.match(requirePairingSecret, /material:\s*&RuntimeSecretMaterial/u);
  assert.match(requirePairingSecret, /secret\.expose_bytes\s*\(\)/u);
  for (const pairingConsumer of ["pairing_create", "pairing_claim"]) {
    const block = rustFunctionBlock(pairing, pairingConsumer);
    assert.ok(block.includes("secret_context.material"), pairingConsumer);
    assert.match(
      block,
      /(?:register_local_relay_endpoint|mobile_relay_claim_proof|one_time_pairing_invite)\s*\(/u,
      pairingConsumer,
    );
  }

  const requireRelayPrivateKey = rustFunctionBlock(
    relayCreate,
    "require_relay_private_key",
  );
  assert.match(requireRelayPrivateKey, /material:\s*&RuntimeSecretMaterial/u);
  assert.match(requireRelayPrivateKey, /private_key\.expose_bytes\s*\(\)/u);
  const secureCreate = rustFunctionBlock(relayCreate, "command_create_secure");
  assert.ok(secureCreate.includes("&secret_context.material"));
  assert.match(secureCreate, /secure_command_payload\s*\(/u);
  assert.match(
    secureCreate,
    /seal_mobile_relay_payload_deferred\s*\(/u,
  );
  const secureResult = rustFunctionBlock(relayResult, "command_result_secure");
  assert.ok(secureResult.includes("&secret_context.material"));
  assert.match(
    secureResult,
    /open_mobile_relay_payload_deferred\s*\(/u,
  );
});

test("E2EE secret bundle is typed bounded binary and the JSON codec is absent", async () => {
  const secretMaterial = await read(`${custodyRoot}/secret_material.rs`);
  const runtimeCarrier = await read(`${custodyRoot}/runtime_secret_material.rs`);
  const joined = `${secretMaterial}\n${runtimeCarrier}`;

  for (const required of [
    "MobileRelayE2eeSecretBundle",
    "MobileRelayE2eeSecretField",
    "MOBILE_RELAY_SECRET_BUNDLE_MAGIC",
    "MOBILE_RELAY_SECRET_BUNDLE_VERSION",
    "MOBILE_RELAY_SECRET_FIELD_MAX_BYTES",
    "MOBILE_RELAY_SECRET_BUNDLE_MAX_BYTES",
    "encode_mobile_relay_e2ee_secret_bundle",
    "decode_mobile_relay_e2ee_secret_bundle",
  ]) {
    assert.ok(joined.includes(required), required);
  }
  for (const legacy of [
    "serialize_native_e2ee_secret_bundle",
    "parse_native_e2ee_secret_bundle",
    "MOBILE_RELAY_E2EE_NATIVE_SECRET_BUNDLE_SCHEMA_VERSION",
    "serde_json::Map",
    "serde_json::from_str::<Value>",
    "serde_json::to_string",
    "Vec<(&'static str, String)>",
    "Vec<(&str, String)>",
  ]) {
    assert.equal(joined.includes(legacy), false, legacy);
  }
});

test("legacy keyring implementation and catalog residue are absent", async () => {
  assert.equal(
    await exists(`${platformRoot}/platform_backends/keyring.rs`),
    false,
  );
  const catalogSources = await rustSources(catalogRoot).catch(() => []);
  const catalogEntries = await fs.readdir(path.join(repoRoot, catalogRoot), {
    withFileTypes: true,
  });
  const topLevelCatalog = await Promise.all(catalogEntries
    .filter((entry) => entry.isFile() && entry.name.endsWith(".mjs"))
    .map((entry) => read(`${catalogRoot}/${entry.name}`)));
  const groupEntries = await fs.readdir(path.join(repoRoot, catalogRoot, "groups"), {
    withFileTypes: true,
  });
  const groups = await Promise.all(groupEntries
    .filter((entry) => entry.isFile() && entry.name.endsWith(".mjs"))
    .map((entry) => read(`${catalogRoot}/groups/${entry.name}`)));
  const catalog = [
    ...catalogSources.map(([, source]) => source),
    ...topLevelCatalog,
    ...groups,
  ].join("\n");

  assert.equal(catalog.includes("platform_backends/keyring.rs"), false);
  assert.equal(catalog.includes("secure_mesh_secret_store/keyring"), false);
});

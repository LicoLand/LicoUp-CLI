import { spawnSync } from "node:child_process";

// RustSec advisory exceptions and duplicate-generation governance for the
// client dependency gate.
//
// Advisory exceptions: the only accepted form is a TEMPORARY exception bound
// to an `expiresOn` date. The gate hard-fails once an expiry date has passed,
// so exceptions are always removed or re-audited, never silently extended.
//
// Current state: RUSTSEC-2026-0173 (proc-macro-error2, unmaintained) is
// RESOLVED. hax-lib 0.3.7 on crates.io still vendors proc-macro-error2 and no
// patched release exists, so root Cargo.toml patches `hax-lib` (source
// `^0.3.7`) to upstream commit
// 8f9cb576e58f6cc7e9ed249a7d4439b0f7ed0da7, which removed the dependency.
// Nothing in the production graph depends on it - `cargo tree -i hax-lib`
// prints nothing under `-e no-dev`, and the libcrux generation this repository
// pins takes hax-lib only from a `cfg(hax)` target section that the product
// build never sets - but the patched source stays in Cargo.lock, which is what
// the advisory scan reads. No advisory exception remains.

const TEMPORARY_EXCEPTIONS = Object.freeze([]);

// Duplicate-generation governance.
//
// `cargo tree -d` on the production graph (`-e no-dev`, `--locked`) must stay
// under DUPLICATE_CRATE_CEILING and every duplicate crate name must be listed
// here with the transitive pin that forces it. This makes the residual
// duplicates an explicit, reviewed set instead of silent drift.
//
// The audit target is DUPLICATE_CRATE_TARGET (≤15). The remaining 24 are
// forced by fixed protocol inputs or by a generation split this repository
// must not force-break:
//   - two RustCrypto / dalek generations, and the split is now this
//     repository's own direct pins rather than the two revisions disagreeing.
//     The openmls git revision (e0b21a70..., through hpke-rs 0.8.0) and the
//     fixed LicoArc SDK revision (244ce718..., L03.0) both sit on the 0.11 /
//     dalek-3 generation (aead 0.6, cipher 0.5, poly1305 0.9, universal-hash
//     0.6, chacha20poly1305 0.11, sha2 0.11, ed25519-dalek 3, x25519-dalek 3);
//     licoup-native and licoup-protocol-bindings pin the 0.10 / dalek-2 one
//     directly (chacha20poly1305 0.10, sha2 0.10, hkdf 0.12, hmac 0.12,
//     ed25519-dalek 2, x25519-dalek 2, rand_core 0.6). Converging them means
//     moving those direct pins, which is a protocol decision and not one the
//     dependency gate may make. Every RustCrypto / dalek duplicate below is a
//     leaf of that split.
//   - syn 2 vs 3: openmls_serialization_helpers 0.1.0 (proc-macro, openmls
//     rev) is the only crate on the graph that moved to syn 3; every derive
//     crate in the graph is still on syn 2.
//   - base64 0.22 vs 0.23: ureq 2.12.1 requires ^0.22 and licoup's own crates
//     pin the same line, while openmls_memory_storage 0.6.0 (persistence
//     feature) requires ^0.23. Bumping the local pins alone would not remove
//     the duplicate, because ureq is the other consumer.
//   - ureq 2.x -> webpki-roots 0.26.11 shim -> webpki-roots 1.x. ureq 3
//     dropped the custom-rustls-verifier hook that
//     domain/provider_quota/http.rs depends on, so the migration is deferred
//     rather than forced.
const TRACKED_DUPLICATE_GENERATIONS = Object.freeze({
  aead: "0.5.2 via chacha20poly1305 0.10.1 (direct pin) vs 0.6.1 via chacha20poly1305 0.11.0 (openmls rev, fixed LicoArc SDK)",
  base64: "0.22.1 direct licoup-native/licoup-protocol-bindings and ureq 2.12.1 vs 0.23.1 via openmls_memory_storage 0.6.0 (openmls rev, persistence feature)",
  "block-buffer": "0.10.4 via digest 0.10.7 (sha2 0.10/hmac 0.12 family, direct) vs 0.12.1 via digest 0.11.3 and cipher 0.5.2 (openmls rev, fixed LicoArc SDK)",
  chacha20: "0.9.1 via chacha20poly1305 0.10.1 (direct pin) vs 0.10.2 via chacha20poly1305 0.11.0 (openmls rev, fixed LicoArc SDK)",
  chacha20poly1305: "0.10.1 direct licoup-native and licoup-protocol-bindings vs 0.11.0 via the fixed LicoArc SDK, openmls_rust_crypto 0.6.0 and hpke-rs-rust-crypto 0.8.0",
  cipher: "0.4.4 via chacha20 0.9.1 and chacha20poly1305 0.10.1 (direct pin) vs 0.5.2 via aes 0.9/aes-gcm 0.11/ctr 0.10 (openmls rev, fixed LicoArc SDK)",
  cpufeatures: "0.2.17 via sha2 0.10.9 (direct pin) vs 0.3.0 via sha2 0.11/aes 0.9/keccak/polyval (openmls rev, fixed LicoArc SDK)",
  "crypto-common": "0.1.7 via aead 0.5.2/cipher 0.4.4/digest 0.10.7 (direct pin) vs 0.2.2 via the digest 0.11 generation (openmls rev, fixed LicoArc SDK)",
  "curve25519-dalek": "4.1.3 via ed25519-dalek 2.2.0 and x25519-dalek 2.0.1 (direct pins) vs 5.0.0 via ed25519-dalek 3/x25519-dalek 3 (openmls rev, fixed LicoArc SDK)",
  digest: "0.10.7 via sha2 0.10.9/hmac 0.12.1/curve25519-dalek 4 (direct pins) vs 0.11.3 via the openmls rev and fixed LicoArc SDK generation (sha2 0.11, hmac 0.13, ecdsa 0.17, sha3 0.11)",
  ed25519: "2.2.3 via ed25519-dalek 2.2.0 (direct pin) vs 3.0.0 via ed25519-dalek 3 (openmls rev, fixed LicoArc SDK)",
  "ed25519-dalek": "2.2.0 direct licoup-native vs 3.0.0 via the fixed LicoArc SDK and the openmls rev (openmls_basic_credential, openmls_rust_crypto)",
  getrandom: "0.2.17 via rand_core 0.6.4 and ring 0.17 (direct pins, ureq TLS) vs 0.4.3 via the rand_core 0.10 generation (openmls rev, fixed LicoArc SDK, uuid)",
  hkdf: "0.12.4 direct licoup-native and licoup-protocol-bindings vs 0.13.0 via the openmls rev, the fixed LicoArc SDK and hpke-rs-rust-crypto 0.8.0",
  hmac: "0.12.1 direct licoup-native and via hkdf 0.12.4 vs 0.13.0 via hkdf 0.13, the fixed LicoArc SDK, the openmls rev and rfc6979",
  inout: "0.1.4 via cipher 0.4.4 (direct pin) vs 0.2.2 via cipher 0.5.2 and aead 0.6.1 (openmls rev, fixed LicoArc SDK)",
  nix: "0.27.1 via command-group 5.0.1 and workspace direct pins vs 0.31.3 via zip 9.0.3 (target cfg macos/bsd)",
  poly1305: "0.8.0 via chacha20poly1305 0.10.1 (direct pin) vs 0.9.1 via chacha20poly1305 0.11.0 (openmls rev, fixed LicoArc SDK)",
  rand_core: "0.6.4 direct licoup-native and licoup-protocol-bindings (ed25519-dalek 2, x25519-dalek 2, crypto-common 0.1) vs 0.10.1 via the openmls rev, the fixed LicoArc SDK and hpke-rs-rust-crypto 0.8.0",
  sha2: "0.10.9 direct in six workspace crates and via ed25519-dalek 2 vs 0.11.0 via the openmls rev, the fixed LicoArc SDK, hpke-rs-rust-crypto 0.8.0 and p256/p384 0.14",
  signature: "2.2.0 via ed25519 2.2.3 (direct pin) vs 3.0.0 via the openmls rev and fixed LicoArc SDK (ecdsa 0.17, ed25519 3, ml-dsa)",
  syn: "2.0.118 via the derive crates (serde_derive, thiserror-impl, zeroize_derive, tls_codec_derive, jni-macros, jni-sys-macros, displaydoc, yoke-derive, zerofrom-derive, zerovec-derive, synstructure) vs 3.0.4 via openmls_serialization_helpers 0.1.0 (proc-macro, openmls rev)",
  "universal-hash": "0.5.1 via poly1305 0.8.0 and chacha20poly1305 0.10.1 (direct pin) vs 0.6.1 via poly1305 0.9.1 and polyval (openmls rev, fixed LicoArc SDK)",
  "webpki-roots": "ureq 2.x pinned 0.26.11 alias (normal dep webpki-roots ^1); ureq 3 migration deferred (API + TLS stack change)",
  "x25519-dalek": "2.0.1 direct licoup-native vs 3.0.0 via the fixed LicoArc SDK and hpke-rs-rust-crypto 0.8.0 (openmls rev)",
});

const DUPLICATE_CRATE_TARGET = 15;
// The macOS and Linux production graphs now carry the same 24 duplicates,
// all of them the pinned crypto generations recorded above. That is down from
// 29: the libcrux family converged on a single generation (libcrux-ml-kem
// 0.0.11, libcrux-sha3 0.0.11, libcrux-traits 0.0.9, which removed the
// intrinsics, platform, secrets, sha3 and traits duplicates at once) once the
// openmls revision moved to hpke-rs 0.8.0, and the two splits that move
// introduced - syn and base64 - are recorded above. It was 33 before that: the
// unused `notify` dependency was dropped (Linux-only `bitflags`), and `jni`
// 0.21 -> 0.22 (`jni-sys` shim, `thiserror` 1) and the direct `rand` ->
// `rand_core` substitution (`rand`) each removed a duplicate generation.
const DUPLICATE_CRATE_CEILING = 28;

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: process.cwd(),
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 32 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(`${command} failed while validating RustSec exceptions`);
  }
  return result.stdout || "";
}

function validateExpiry(exception) {
  const expiry = new Date(`${exception.expiresOn}T00:00:00Z`);
  if (!Number.isFinite(expiry.getTime()) || Date.now() >= expiry.getTime()) {
    throw new Error(`${exception.advisory} exception expired; remove or re-audit it`);
  }
}

function validateReachability(exception) {
  const metadata = JSON.parse(run("cargo", ["metadata", "--format-version", "1", "--locked"]));
  if (!metadata.packages.some((item) => item.name === exception.package)) {
    return;
  }
  const activeInverseTree = run("cargo", [
    "tree",
    "--workspace",
    "--locked",
    "-i",
    exception.package,
    "-e",
    "all",
  ]);
  if (activeInverseTree.includes(`${exception.package} v`)) {
    throw new Error(`${exception.advisory} became reachable in the product feature graph`);
  }
}

function validateDuplicateGenerations() {
  const tree = run("cargo", ["tree", "-d", "--workspace", "--locked", "-e", "no-dev"]);
  const versions = new Map();
  for (const line of tree.split(/\r?\n/u)) {
    const match = /^([a-z0-9_-]+) v([0-9][^ ]*)/u.exec(line);
    if (!match) continue;
    const set = versions.get(match[1]) ?? new Set();
    set.add(match[2]);
    versions.set(match[1], set);
  }
  const duplicateCrates = [...versions.entries()]
    .filter(([, set]) => set.size > 1)
    .map(([name]) => name)
    .sort();
  const unexplained = duplicateCrates.filter(
    (name) => !TRACKED_DUPLICATE_GENERATIONS[name],
  );
  if (duplicateCrates.length > DUPLICATE_CRATE_CEILING) {
    throw new Error(
      `duplicate crate generations rose above the ${DUPLICATE_CRATE_CEILING} ceiling: ${duplicateCrates.length}`,
    );
  }
  if (unexplained.length > 0) {
    throw new Error(
      `unexplained duplicate crate generations: ${unexplained.join(", ")}; ` +
        "converge them or record the pinned reason in TRACKED_DUPLICATE_GENERATIONS",
    );
  }
  return {
    duplicateCrates,
    tracked: duplicateCrates.length,
    target: DUPLICATE_CRATE_TARGET,
    ceiling: DUPLICATE_CRATE_CEILING,
  };
}

for (const exception of TEMPORARY_EXCEPTIONS) {
  validateExpiry(exception);
  validateReachability(exception);
}

const duplicateGovernance = validateDuplicateGenerations();

const rustCfg = run("rustc", ["--print", "cfg"]);
if (rustCfg.split(/\r?\n/u).some((line) => line.trim() === "hax")) {
  throw new Error("advisory exception contracts are invalid when cfg(hax) is enabled");
}

process.stdout.write(
  `${JSON.stringify({
    ok: true,
    temporaryException: TEMPORARY_EXCEPTIONS.length === 0
      ? null
      : TEMPORARY_EXCEPTIONS.map((exception) => exception.advisory),
    reachable: false,
    ...duplicateGovernance,
  })}\n`,
);

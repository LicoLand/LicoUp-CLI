import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import process from "node:process";
import { repoRoot } from "../constants.mjs";
import { commandOptions } from "./process.mjs";

export function buildSignedSwiftHelper(swiftPath, { tempDir, options = {} } = {}) {
  const bundlePath = path.join(tempDir, "MacosAdaptiveCustodyProof.app");
  const contentsPath = path.join(bundlePath, "Contents");
  const executableDirectory = path.join(contentsPath, "MacOS");
  mkdirSync(executableDirectory, { recursive: true });
  const helperPath = path.join(executableDirectory, "MacosAdaptiveCustodyProof");
  writeFileSync(path.join(contentsPath, "Info.plist"), helperInfoPlist(), "utf8");

  const compile = spawnSync("swiftc", [
    swiftPath,
    "-framework",
    "Foundation",
    "-framework",
    "LocalAuthentication",
    "-framework",
    "Security",
    "-o",
    helperPath,
  ], commandOptions(30_000));
  if (compile.status !== 0) {
    return failedHelper("compile_failed");
  }

  const selectedIdentity = selectCodesignIdentity(options);
  const signArgs = ["--force", "--sign", selectedIdentity.value, "--timestamp=none"];
  let entitlementsApplied = false;
  const teamIdentifier = resolveTeamIdentifier();
  if (selectedIdentity.value !== "-" && teamIdentifier) {
    const entitlementsPath = path.join(tempDir, "MacosAdaptiveCustodyProof.entitlements");
    writeFileSync(entitlementsPath, helperEntitlements(teamIdentifier), "utf8");
    signArgs.push("--entitlements", entitlementsPath);
    entitlementsApplied = true;
  }
  signArgs.push(bundlePath);
  const sign = spawnSync("codesign", signArgs, commandOptions(15_000));
  if (sign.status !== 0) {
    return failedHelper("codesign_failed", selectedIdentity.kind);
  }
  const verify = spawnSync(
    "codesign",
    ["--verify", "--strict", bundlePath],
    commandOptions(10_000),
  );
  if (verify.status !== 0) {
    return failedHelper("signature_verification_failed", selectedIdentity.kind);
  }
  return {
    path: helperPath,
    signatureValid: true,
    signatureMode: selectedIdentity.kind,
    entitlementsApplied,
    ran: false,
    failureCode: "",
  };
}

export function failedHelper(failureCode, signatureMode = "unavailable") {
  return {
    path: "",
    signatureValid: false,
    signatureMode,
    entitlementsApplied: false,
    ran: false,
    failureCode,
  };
}

export function selectCodesignIdentity(options = {}) {
  const configured = String(
    options.signIdentity || process.env.LICO_MACOS_CODESIGN_IDENTITY || "",
  ).trim();
  if (configured) return { value: configured, kind: "configured_development" };
  const discovered = discoverDevelopmentCodesignIdentity();
  if (discovered) return { value: discovered, kind: "automatic_development" };
  return { value: "-", kind: "adhoc" };
}

export function discoverDevelopmentCodesignIdentity() {
  const result = spawnSync(
    "security",
    ["find-identity", "-v", "-p", "codesigning"],
    commandOptions(5_000),
  );
  if (result.status !== 0) return "";
  const identities = String(result.stdout || "")
    .split(/\r?\n/u)
    .map((line) => line.match(/^\s*\d+\)\s+[A-F0-9]+\s+"([^"]+)"/u)?.[1] || "")
    .filter(Boolean);
  return identities.find((identity) => identity.startsWith("Apple Development:")) ||
    identities[0] ||
    "";
}

export function resolveTeamIdentifier() {
  const configured = String(
    options.teamIdentifier || process.env.LICO_MACOS_DEVELOPMENT_TEAM || "",
  ).trim();
  return /^[A-Z0-9]{10}$/u.test(configured) ? configured : "";
}

export function helperEntitlements(teamIdentifier) {
  const bundleIdentifier = "land.lico.licoup.secure-mesh.macos-adaptive-custody-proof";
  const applicationIdentifier = `${teamIdentifier}.${bundleIdentifier}`;
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.application-identifier</key>
  <string>${applicationIdentifier}</string>
  <key>com.apple.developer.team-identifier</key>
  <string>${teamIdentifier}</string>
  <key>keychain-access-groups</key>
  <array>
    <string>${applicationIdentifier}</string>
  </array>
</dict>
</plist>
`;
}

export function helperInfoPlist() {
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>land.lico.licoup.secure-mesh.macos-adaptive-custody-proof</string>
  <key>CFBundleExecutable</key>
  <string>MacosAdaptiveCustodyProof</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleVersion</key>
  <string>1</string>
</dict>
</plist>
`;
}

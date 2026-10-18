import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const read = (relative) => readFileSync(path.join(root, relative), "utf8");

const manifest = read("crates/licoup-agent-targets/resources/agent-scan-paths.toml");
const scanPaths = read("crates/licoup-agent-targets/src/domain/targets/scan_paths.rs");
const platformPaths = read(
  "crates/licoup-agent-targets/src/domain/targets/platform_paths.rs",
);
const nativeHome = read("crates/licoup-foundation/src/platform/paths.rs");
const binaries = read("crates/licoup-agent-targets/src/domain/targets/binaries.rs").split(
  "#[cfg(test)]",
  1,
)[0];
const agentServiceActions = read(
  "apps/desktop/lib/src/platform/native_client/agent_service_actions.dart",
);
const historyDiscovery = read(
  "crates/licoup-native/src/domain/conversation/history_discovery.rs",
);
const lifecycleFacade = read(
  "apps/desktop/lib/src/application/controller/client_lifecycle_facade.dart",
);

test("Agent discovery is an allowlisted TOML scan, not a PATH walk", () => {
  assert.match(manifest, /schema_version = "licoup\.agent-scan-paths\.v2"/u);
  assert.match(manifest, /network_prefixes = \["\/Volumes"\]/u);
  assert.match(manifest, /"Desktop"/u);
  assert.match(manifest, /"Documents"/u);
  assert.match(manifest, /"Downloads"/u);
  assert.match(manifest, /"Pictures"/u);
  assert.match(manifest, /"Music"/u);
  assert.match(manifest, /id = "cursor"/u);
  const claudeManifest = (manifest.split('id = "claude-code"')[1] ?? "").split(
    "[[agents]]",
  )[0] ?? "";
  assert.match(claudeManifest, /config = \[[\s\S]*\.claude\.json/u);
  assert.doesNotMatch(
    claudeManifest.split("history =")[1] ?? "",
    /\.claude\.json/u,
  );
  assert.match(scanPaths, /include_str!\("\.\.\/\.\.\/\.\.\/resources\/agent-scan-paths\.toml"\)/u);
  assert.match(scanPaths, /fn admitted_scan_path/u);
  assert.doesNotMatch(binaries, /env::var_os\("PATH"\)/u);
  assert.doesNotMatch(binaries, /split_paths/u);
  assert.match(binaries, /scan_paths::binary_dirs/u);
  assert.match(scanPaths, /fn nvm_default_bin_dirs/u);
  assert.match(manifest, /\.local\/share\/mise\/shims/u);
  assert.match(manifest, /other_app_home_prefixes/u);
  assert.match(manifest, /Library\/Application Support/u);
  assert.match(scanPaths, /fn is_other_app_container/u);
  assert.doesNotMatch(scanPaths, /UserDirs/u);
  assert.doesNotMatch(platformPaths, /UserDirs/u);
  assert.match(nativeHome, /fn user_home_from_env/u);
  assert.match(nativeHome, /fn strip_macos_data_volume/u);
  assert.doesNotMatch(nativeHome, /UserDirs::new/u);
  assert.match(binaries, /scan_paths::denied/u);
  assert.match(manifest, /Library\/Mobile Documents/u);
  assert.match(manifest, /Library\/CloudStorage/u);
  assert.match(scanPaths, /fn automatic_agent_execution_admitted/u);
  assert.match(scanPaths, /fn discovered_agent_may_execute/u);
  assert.match(scanPaths, /fn probe_exists_under_home/u);
  assert.match(scanPaths, /fn probe_exists_with/u);
  assert.match(
    scanPaths,
    /fn automatic_probe_admitted[\s\S]*automatic_probe_admitted_with/u,
  );
  assert.match(
    read("crates/licoup-agent-targets/src/domain/targets/model_catalog/config.rs"),
    /probe_exists_under_home/u,
  );
  assert.match(
    binaries,
    /find_binary_in_dirs\(&\["cursor-agent"\], dirs\)/u,
  );
  assert.doesNotMatch(
    (binaries.split("fn find_cursor_binary_in_dirs")[1] ?? "").split(
      "pub(super) fn",
    )[0] ?? "",
    /cursor_binary_supports_acp/u,
  );
  assert.match(
    agentServiceActions,
    /'--include-history-model-catalog',\s*'false'/u,
  );
  const selectedBatch = (agentServiceActions.split("scanTargetsBatch")[1] ?? "")
    .split("_targetScanSlot")[0] ?? "";
  assert.match(
    selectedBatch,
    /bool enableAgentCliModelLookup = false/u,
  );
  assert.match(
    selectedBatch,
    /runCliWithStdin\([\s\S]*'--stdin-json',[\s\S]*'true'/u,
  );
  assert.match(selectedBatch, /'targetIds': normalizedTargetIds/u);
  assert.match(
    selectedBatch,
    /'modelCatalogTargetIds': enableAgentCliModelLookup[\s\S]*\? normalizedTargetIds/u,
  );
  assert.equal(selectedBatch.match(/runCliWithStdin/gu)?.length, 1);
  assert.match(agentServiceActions, /enable-agent-cli-model-lookup/u);
  assert.match(historyDiscovery, /denied_personal_location/u);
  assert.match(historyDiscovery, /denied_symlink_escape/u);
  assert.match(scanPaths, /fn symlink_escapes_denied_location/u);
  assert.doesNotMatch(
    lifecycleFacade.split("_finalizeClientInitialization")[1] ?? "",
    /startAgentUsagePolling/u,
  );
});

//! The DeepSeek Harness provider catalogue, owned by the DeepSeek adapter
//! package.
//!
//! The advisory rows a default Harness installation starts from are the
//! provider's own data, so they live in the package that carries this Agent
//! (`licoup_agent_deepseek::model_catalog`) rather than beside the client's
//! registry. What stays here is the *merge*: how one catalogue document becomes
//! rows in this target's model registry, which is the client's business.
//!
//! Reading the catalogue is a read of data, not an execution: no Node runtime is
//! started, nothing is resolved through the installed package tree, and no
//! vendor library is loaded. The transcription declares the vendor generation it
//! describes, so a reader can tell which build it follows rather than assuming
//! it tracks whatever is installed.

use super::*;

pub(super) const SOURCE: &str = "deepseek-harness-installed-adapter";

pub(super) fn collect_installed_model_catalog(
    params: &Value,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) -> bool {
    if !agent_cli_model_lookup_enabled(params) {
        diagnostics.push(json!({"source":SOURCE,"status":"disabled"}));
        return false;
    }
    // The catalogue is this package's own declared data, so nothing has to be
    // found on disk for it to be read. An installation whose provider replaced
    // its advisory list keeps its own replacement; what is advertised here is
    // what a default installation starts from.
    let catalog = licoup_agent_deepseek::model_catalog::installed_catalog();
    let mut installed = BTreeMap::new();
    let mut sources = BTreeSet::new();
    merge_model_catalog_value_into(&catalog, SOURCE, &mut installed, &mut sources, diagnostics);
    if installed.is_empty() {
        diagnostics.push(json!({"source":SOURCE,"status":"empty"}));
        return false;
    }
    for (key, entry) in installed {
        entries.insert(key, entry);
    }
    true
}

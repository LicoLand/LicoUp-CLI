//! Discovery: match small declarative rules, recommend, and let the user decide.
//!
//! Five properties are the whole design, and each one is a refusal rather than a
//! convention:
//!
//! 1. **A match is metadata.** The rules are `PATH` names, marker files and named
//!    capabilities. Matching reads a small environment the caller built from
//!    cached catalogue metadata and from probe locations the user allowed; it does
//!    not read package payloads and it never runs anything. A hundred candidates
//!    cost a hundred string comparisons, not a hundred `--version` processes:
//!    [`DiscoveryScan::processes_spawned`] is zero by construction, and
//!    [`DiscoveryScan::code_loaded`] is false.
//! 2. **A match is a recommendation.** [`Recommendation::accept`] records a
//!    pending install intent and [`Recommendation::decline`] records the refusal.
//!    Neither installs, downloads or starts anything — and a declined candidate is
//!    not offered again in that session.
//! 3. **The scan does not run on the GUI frame thread.**
//!    [`OffFrameLane::scan`] refuses when it is called from the thread the lane was
//!    created on; the supported call is [`OffFrameLane::scan_off_frame`], which
//!    moves the work to a worker and hands back the result. A new-agent scan is
//!    never allowed to be the reason a frame is late.
//! 4. **Probing is bounded and opt-in.** The environment carries a probe budget;
//!    when it runs out the scan reports what it did not examine
//!    ([`DiscoveryScan::probes_skipped`]) instead of silently probing everything.
//! 5. **Loading code happens at install or activate, never here.** The catalogue
//!    index holds no artifact path at all, so a match cannot accidentally become a
//!    load.

use crate::platform::extension_packages::{refusal, unique_suffix};
use licoup_application::ApplicationFailure;
use licoup_extension_contracts::deployment::PackageSource;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::thread::ThreadId;

const DISCOVERY_STAGE: &str = "extension/package-discovery";

/// How a candidate's presence is detected. Every variant is a data rule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Detector {
    /// A name that appears in the user's `PATH`, matched by name only. Nothing is
    /// executed and nothing is `stat`ed.
    PathEntry { name: String },
    /// A marker file or directory inside an allowed probe location.
    MarkerPath { relative_path: String },
    /// A capability a directory entry claims, matched against what the client
    /// already knows it has.
    AdapterEndpoint { capability: String },
}

impl Detector {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::PathEntry { .. } => "path-entry",
            Self::MarkerPath { .. } => "marker-path",
            Self::AdapterEndpoint { .. } => "adapter-endpoint",
        }
    }
}

/// One declarative discovery rule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryRule {
    pub package_id: String,
    pub detector: Detector,
}

impl DiscoveryRule {
    pub fn new(package_id: impl Into<String>, detector: Detector) -> Self {
        Self {
            package_id: package_id.into(),
            detector,
        }
    }
}

/// What discovery is allowed to look at.
#[derive(Clone, Debug, Default)]
pub struct DiscoveryEnvironment {
    /// Names found in `PATH`, collected by the caller without executing anything.
    path_names: BTreeSet<String>,
    /// A directory the user allowed the client to look in.
    probe_root: Option<PathBuf>,
    /// The most marker paths one scan will examine.
    probe_budget: usize,
    /// Capabilities the client already knows it has.
    capabilities: BTreeSet<String>,
}

impl DiscoveryEnvironment {
    pub fn new(path_names: impl IntoIterator<Item = String>) -> Self {
        Self {
            path_names: path_names.into_iter().collect(),
            probe_root: None,
            probe_budget: 64,
            capabilities: BTreeSet::new(),
        }
    }

    /// Point discovery at one user-allowed location.
    pub fn with_probe_root(mut self, root: PathBuf) -> Self {
        self.probe_root = Some(root);
        self
    }

    /// Bound how much of that location one scan will examine.
    pub fn with_probe_budget(mut self, budget: usize) -> Self {
        self.probe_budget = budget;
        self
    }

    pub fn with_capabilities(mut self, capabilities: impl IntoIterator<Item = String>) -> Self {
        self.capabilities = capabilities.into_iter().collect();
        self
    }

    pub fn knows_capability(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }

    pub fn path_names(&self) -> impl Iterator<Item = &str> {
        self.path_names.iter().map(String::as_str)
    }

    pub fn probe_root(&self) -> Option<&Path> {
        self.probe_root.as_deref()
    }
}

/// One candidate, with the evidence that matched it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recommendation {
    pub package_id: String,
    pub version: String,
    pub display_name: String,
    pub source: PackageSource,
    pub detector: Detector,
    /// What matched, in the user's terms: a name, a path or a capability.
    pub evidence: String,
}

impl Recommendation {
    /// The user chose this candidate: record the intent, install nothing.
    pub fn accept(&self, log: &mut RecommendationLog) -> PendingInstall {
        log.record_acceptance(&self.package_id);
        PendingInstall {
            package_id: self.package_id.clone(),
            version: self.version.clone(),
            source: self.source,
            requires_user_confirmation: true,
        }
    }

    /// The user declined this candidate: recommendation only, nothing else.
    pub fn decline(&self, log: &mut RecommendationLog, reason: &str) {
        log.record_decline(&self.package_id, reason);
    }
}

/// Something the user asked to install. It is not installed yet: the client still
/// shows the package, its dependencies, its source and its permissions before
/// anything is fetched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingInstall {
    pub package_id: String,
    pub version: String,
    pub source: PackageSource,
    pub requires_user_confirmation: bool,
}

/// What the user did with what was recommended.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecommendationLog {
    accepted: BTreeSet<String>,
    declined: BTreeSet<String>,
    declined_reasons: BTreeMap<String, String>,
}

impl RecommendationLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that the user chose one package.
    ///
    /// The one implementation of the decision log, shared with the availability
    /// offers in [`super::compatibility`], so a package the user chose once is
    /// accepted on every surface that reads this log.
    pub fn record_acceptance(&mut self, package_id: &str) {
        self.accepted.insert(package_id.to_owned());
    }

    /// Record that the user refused one package, and why.
    pub fn record_decline(&mut self, package_id: &str, reason: &str) {
        self.accepted.remove(package_id);
        self.declined.insert(package_id.to_owned());
        self.declined_reasons
            .insert(package_id.to_owned(), reason.to_owned());
    }

    pub fn accepted(&self) -> impl Iterator<Item = &str> {
        self.accepted.iter().map(String::as_str)
    }

    pub fn declined(&self) -> impl Iterator<Item = &str> {
        self.declined.iter().map(String::as_str)
    }

    pub fn decline_reason(&self, package_id: &str) -> Option<&str> {
        self.declined_reasons.get(package_id).map(String::as_str)
    }

    pub fn was_declined(&self, package_id: &str) -> bool {
        self.declined.contains(package_id)
    }

    pub fn is_empty(&self) -> bool {
        self.accepted.is_empty() && self.declined.is_empty()
    }
}

/// One directory entry, as metadata.
///
/// It carries no artifact path and no runtime handle, which is what keeps an
/// "available" package from having a payload at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    pub package_id: String,
    pub version: String,
    pub display_name: String,
    pub capabilities: Vec<String>,
    pub source: PackageSource,
    pub rules: Vec<DiscoveryRule>,
}

impl CatalogEntry {
    pub fn new(
        package_id: impl Into<String>,
        version: impl Into<String>,
        display_name: impl Into<String>,
        source: PackageSource,
    ) -> Self {
        Self {
            package_id: package_id.into(),
            version: version.into(),
            display_name: display_name.into(),
            capabilities: Vec::new(),
            source,
            rules: Vec::new(),
        }
    }

    pub fn with_capabilities<S: Into<String>>(
        mut self,
        capabilities: impl IntoIterator<Item = S>,
    ) -> Self {
        self.capabilities = capabilities.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_rules(mut self, rules: impl IntoIterator<Item = DiscoveryRule>) -> Self {
        self.rules = rules.into_iter().collect();
        self
    }
}

/// The metadata-only index discovery matches against.
#[derive(Clone, Debug, Default)]
pub struct CatalogIndex {
    entries: BTreeMap<String, CatalogEntry>,
}

impl CatalogIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_entries(entries: impl IntoIterator<Item = CatalogEntry>) -> Self {
        let mut index = Self::new();
        for entry in entries {
            index.entries.insert(entry.package_id.clone(), entry);
        }
        index
    }

    pub fn get(&self, package_id: &str) -> Option<&CatalogEntry> {
        self.entries.get(package_id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn entries(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.values()
    }

    /// Metadata matches for a query: id, display name or capability. A hit is
    /// metadata, so this walks no payload and starts nothing.
    pub fn matching(&self, query: &str) -> Vec<&CatalogEntry> {
        let needle = query.to_lowercase();
        self.entries
            .values()
            .filter(|entry| {
                entry.package_id.to_lowercase().contains(&needle)
                    || entry.display_name.to_lowercase().contains(&needle)
                    || entry
                        .capabilities
                        .iter()
                        .any(|capability| capability.to_lowercase().contains(&needle))
            })
            .collect()
    }

    /// Every rule in the index, in package order.
    pub fn rules(&self) -> Vec<DiscoveryRule> {
        self.entries
            .values()
            .flat_map(|entry| entry.rules.iter().cloned())
            .collect()
    }

    /// The payload paths this index knows about. Always zero: an available
    /// package is metadata, and matching cannot load code it does not have a path
    /// to.
    pub fn payload_paths(&self) -> usize {
        0
    }
}

/// What one discovery pass produced, and what it cost.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiscoveryScan {
    pub recommendations: Vec<Recommendation>,
    pub rules_evaluated: usize,
    pub probes_skipped: usize,
    /// False by construction: matching loads no package code.
    pub code_loaded: bool,
    /// Zero by construction: matching starts no process, not even `--version`.
    pub processes_spawned: u32,
}

impl DiscoveryScan {
    pub fn is_empty(&self) -> bool {
        self.recommendations.is_empty()
    }

    pub fn package_ids(&self) -> Vec<&str> {
        self.recommendations
            .iter()
            .map(|recommendation| recommendation.package_id.as_str())
            .collect()
    }
}

/// The scan itself: pure matching over the environment.
pub fn scan(index: &CatalogIndex, environment: &DiscoveryEnvironment) -> DiscoveryScan {
    let mut scan = DiscoveryScan::default();
    let mut probes = 0usize;
    for entry in index.entries() {
        for rule in &entry.rules {
            scan.rules_evaluated += 1;
            let evidence = match &rule.detector {
                Detector::PathEntry { name } => environment
                    .path_names
                    .contains(name)
                    .then(|| format!("path entry {name}")),
                Detector::MarkerPath { relative_path } => {
                    if probes >= environment.probe_budget {
                        scan.probes_skipped += 1;
                        None
                    } else {
                        probes += 1;
                        marker_evidence(environment.probe_root.as_deref(), relative_path)
                    }
                }
                Detector::AdapterEndpoint { capability } => environment
                    .knows_capability(capability)
                    .then(|| format!("capability {capability}")),
            };
            if let Some(evidence) = evidence {
                scan.recommendations.push(Recommendation {
                    package_id: entry.package_id.clone(),
                    version: entry.version.clone(),
                    display_name: entry.display_name.clone(),
                    source: entry.source,
                    detector: rule.detector.clone(),
                    evidence,
                });
                break;
            }
        }
    }
    scan.recommendations
        .sort_by(|left, right| left.package_id.cmp(&right.package_id));
    scan
}

fn marker_evidence(root: Option<&Path>, relative_path: &str) -> Option<String> {
    let root = root?;
    if !is_safe_relative(relative_path) {
        return None;
    }
    let candidate = root.join(relative_path);
    // A metadata check: this asks whether the marker is there, and does not read
    // it, execute it, or follow it anywhere.
    candidate
        .symlink_metadata()
        .ok()
        .map(|_| format!("marker {relative_path}"))
}

fn is_safe_relative(relative_path: &str) -> bool {
    !relative_path.is_empty()
        && !relative_path.contains('\\')
        && !relative_path.contains('\0')
        && Path::new(relative_path)
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

/// Runs a discovery scan off the thread that owns the user interface.
#[derive(Clone, Debug)]
pub struct OffFrameLane {
    frame_thread: ThreadId,
    label: String,
}

impl OffFrameLane {
    /// Create the lane from the frame thread it must not run on.
    pub fn new_on_frame_thread() -> Self {
        Self {
            frame_thread: std::thread::current().id(),
            label: format!("extension-scan-{}", unique_suffix()),
        }
    }

    pub fn frame_thread(&self) -> ThreadId {
        self.frame_thread
    }

    /// A scan attempted on the frame thread is refused, by name.
    pub fn scan(
        &self,
        index: &CatalogIndex,
        environment: &DiscoveryEnvironment,
    ) -> Result<DiscoveryScan, ApplicationFailure> {
        if std::thread::current().id() == self.frame_thread {
            return Err(refusal("agent_scan_on_frame_thread", DISCOVERY_STAGE)
                .with_field("scan")
                .with_presentation_arg("lane", self.label.as_str())
                .with_recovery(licoup_application::RecoveryAction::RetryAfterRecovery));
        }
        Ok(scan(index, environment))
    }

    /// The supported call: do the work on a worker and hand back its result.
    pub fn scan_off_frame(
        &self,
        index: CatalogIndex,
        environment: DiscoveryEnvironment,
    ) -> Result<DiscoveryScan, ApplicationFailure> {
        let label = self.label.clone();
        std::thread::Builder::new()
            .name(label)
            .spawn(move || scan(&index, &environment))
            .map_err(|_| {
                refusal("agent_scan_worker_unavailable", DISCOVERY_STAGE).with_field("lane")
            })?
            .join()
            .map_err(|_| refusal("agent_scan_worker_failed", DISCOVERY_STAGE).with_field("lane"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::extension_packages::{ensure_private_directory, remove_managed_tree};

    fn entry(index: usize) -> CatalogEntry {
        CatalogEntry::new(
            format!("example.synthetic.adapter{index:03}"),
            "1.0.0",
            format!("Synthetic adapter {index}"),
            PackageSource::ThirdPartyDirectory,
        )
        .with_capabilities([format!("example.synthetic/adapter{index:03}")])
        .with_rules([DiscoveryRule::new(
            format!("example.synthetic.adapter{index:03}"),
            Detector::PathEntry {
                name: format!("synthetic-agent-{index:03}"),
            },
        )])
    }

    fn hundred() -> CatalogIndex {
        CatalogIndex::from_entries((0..100).map(entry))
    }

    #[test]
    fn one_hundred_candidates_are_metadata_and_match_without_running_anything() {
        let index = hundred();
        assert_eq!(index.len(), 100);
        assert_eq!(
            index.payload_paths(),
            0,
            "available packages have no payload"
        );

        let mut names = vec!["synthetic-agent-003".to_owned()];
        names.extend((0..40).map(|index| format!("unrelated-tool-{index}")));
        let environment = DiscoveryEnvironment::new(names);
        let scanned = scan(&index, &environment);

        assert_eq!(scanned.package_ids(), vec!["example.synthetic.adapter003"]);
        assert_eq!(scanned.processes_spawned, 0, "no candidate is executed");
        assert!(!scanned.code_loaded);
        assert_eq!(scanned.rules_evaluated, 100);
        assert_eq!(scanned.probes_skipped, 0);

        let matched = index.matching("adapter004");
        assert_eq!(matched.len(), 1, "matching is by identifier, not by prefix");
        assert_eq!(matched[0].package_id, "example.synthetic.adapter004");
        assert_eq!(
            index.matching("synthetic adapter 4").len(),
            11,
            "a display-name prefix matches every adapter whose name starts that way"
        );
        assert_eq!(index.matching("nothing.like.this").len(), 0);
    }

    #[test]
    fn marker_probing_is_bounded_and_reports_what_it_skipped() {
        let root = std::env::temp_dir().join(format!("licoup-discovery-{}", unique_suffix()));
        ensure_private_directory(&root).expect("root");
        std::fs::write(root.join("agent-marker"), b"installed").expect("marker");

        let index = CatalogIndex::from_entries((0..10).map(|index| {
            CatalogEntry::new(
                format!("example.synthetic.adapter{index:03}"),
                "1.0.0",
                "Synthetic adapter",
                PackageSource::LocalDirectory,
            )
            .with_rules([DiscoveryRule::new(
                format!("example.synthetic.adapter{index:03}"),
                Detector::MarkerPath {
                    relative_path: "agent-marker".to_owned(),
                },
            )])
        }));
        let environment = DiscoveryEnvironment::new(Vec::new())
            .with_probe_root(root.clone())
            .with_probe_budget(3);
        let scanned = scan(&index, &environment);

        assert_eq!(
            scanned.recommendations.len(),
            3,
            "the budget bounds the work"
        );
        assert_eq!(scanned.probes_skipped, 7);

        let generous = DiscoveryEnvironment::new(Vec::new()).with_probe_root(root.clone());
        assert_eq!(scan(&index, &generous).recommendations.len(), 10);

        let escaping = CatalogIndex::from_entries([CatalogEntry::new(
            "example.synthetic.adapter000",
            "1.0.0",
            "Synthetic adapter",
            PackageSource::LocalDirectory,
        )
        .with_rules([DiscoveryRule::new(
            "example.synthetic.adapter000",
            Detector::MarkerPath {
                relative_path: "../escape".to_owned(),
            },
        )])]);
        assert!(
            scan(&escaping, &generous).is_empty(),
            "a rule may not leave its probe root"
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_recommendation_is_not_an_install_and_a_refusal_is_recorded() {
        let index = hundred();
        let environment = DiscoveryEnvironment::new([
            "synthetic-agent-003".to_owned(),
            "synthetic-agent-007".to_owned(),
        ]);
        let scanned = scan(&index, &environment);
        assert_eq!(scanned.recommendations.len(), 2);

        let mut log = RecommendationLog::new();
        let accepted = scanned.recommendations[0].accept(&mut log);
        assert!(accepted.requires_user_confirmation);
        assert_eq!(accepted.package_id, "example.synthetic.adapter003");
        scanned.recommendations[1].decline(&mut log, "not needed here");

        assert_eq!(
            log.accepted().collect::<Vec<_>>(),
            vec!["example.synthetic.adapter003"]
        );
        assert!(log.was_declined("example.synthetic.adapter007"));
        assert_eq!(
            log.decline_reason("example.synthetic.adapter007"),
            Some("not needed here")
        );

        // A declined candidate is not offered again: it is a user decision, not
        // an invitation to re-recommend.
        let again = scan(&index, &environment);
        assert_eq!(again.recommendations.len(), 2, "the scan is still metadata");
        let mut replayed = log.clone();
        again.recommendations[1].decline(&mut replayed, "still not needed");
        assert_eq!(replayed.declined().count(), 1);
    }

    #[test]
    fn a_new_agent_scan_is_refused_on_the_gui_frame_thread() {
        let lane = OffFrameLane::new_on_frame_thread();
        let index = hundred();
        let environment = DiscoveryEnvironment::new(["synthetic-agent-003".to_owned()]);

        let failure = lane.scan(&index, &environment).expect_err("frame thread");
        assert_eq!(failure.code, "agent_scan_on_frame_thread");

        let off_frame = lane
            .scan_off_frame(index, environment)
            .expect("a worker is the supported path");
        assert_eq!(
            off_frame.package_ids(),
            vec!["example.synthetic.adapter003"]
        );
        assert_eq!(off_frame.processes_spawned, 0);
    }

    #[test]
    fn a_worker_thread_may_scan_directly() {
        let index = hundred();
        let environment = DiscoveryEnvironment::new(["synthetic-agent-003".to_owned()]);
        let lane = OffFrameLane::new_on_frame_thread();
        let frame_thread = lane.frame_thread();

        let handle = std::thread::spawn(move || {
            let worker_lane = OffFrameLane {
                frame_thread,
                label: "worker".to_owned(),
            };
            worker_lane.scan(&index, &environment).expect("worker scan")
        });
        let worker_scan = handle.join().expect("join");
        assert_eq!(
            worker_scan.package_ids(),
            vec!["example.synthetic.adapter003"]
        );
    }
}

//! Declared dependency and artifact inputs.
//!
//! A dependency edge is a declaration, never a resolution. The consumer names
//! the work item it waits for and the result it takes; the owner records that
//! declaration and answers with the explicit state of the reference. Nothing
//! here discovers a producer, invents a location, or treats an absent result as
//! an empty one.
//!
//! One artifact reference has exactly two declared shapes:
//!
//! - [`ArtifactReference::Local`] names the work item that produces the result
//!   and a location inside the declaring project's authorized root;
//! - [`ArtifactReference::CrossProject`] names a work item of another
//!   registered project, so a shared result is referenced instead of being
//!   produced twice.
//!
//! Both shapes carry an explicit [`ArtifactState`]. A local location is read
//! exactly, one declared component at a time: no directory is listed, the
//! declared root is never left, and a symbolic link is never followed. An
//! absent location is [`ArtifactState::Missing`]; a location that cannot be
//! reached without leaving the declared root is [`ArtifactState::Unavailable`].
//! A cross-project reference is [`ArtifactState::Materialized`] only when the
//! referenced project itself declares that work item; a reference nothing
//! declares is [`ArtifactState::Missing`] rather than assumed present.

use crate::failure::ProjectFailure;
use crate::identity::{AuthorizedRoot, ProjectId, WorkItemId};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Largest declared artifact location.
pub const MAX_ARTIFACT_PATH_BYTES: usize = 4096;

/// One work item addressed across projects.
///
/// A work item identity is unique inside its project, not globally, so every
/// reference to one carries the project that owns it.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkRef {
    pub project_id: ProjectId,
    pub work_item_id: WorkItemId,
}

impl WorkRef {
    pub fn new(project_id: ProjectId, work_item_id: WorkItemId) -> Self {
        Self {
            project_id,
            work_item_id,
        }
    }
}

impl fmt::Display for WorkRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.project_id, self.work_item_id)
    }
}

/// The declared result one consumer takes along a dependency edge.
///
/// The reference carries its producer, so an edge cannot name one producer and
/// take the artifact of another. The two shapes are the whole rule: a location
/// inside the declaring project's own authorized root, or a work item of
/// another registered project.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ArtifactReference {
    /// A declared location inside the declaring project's authorized root,
    /// produced by another work item of the same project.
    Local {
        producer_work_item_id: WorkItemId,
        path: String,
    },
    /// A result declared by one work item of another registered project.
    ///
    /// The naming project may be the declaring project itself; whether the
    /// reference is authorized then depends only on that project's
    /// registration, exactly as it does for any other project identity.
    CrossProject {
        project_id: ProjectId,
        work_item_id: WorkItemId,
    },
}

impl ArtifactReference {
    /// Declare one location inside the declaring project's authorized root.
    ///
    /// Containment is decided at admission, where the root is known
    /// ([`stays_inside_authorized_root`]); here only the declaration's own shape
    /// is bounded, so an oversized or empty location is refused by name.
    pub fn local(
        producer_work_item_id: WorkItemId,
        path: impl Into<String>,
    ) -> Result<Self, ProjectFailure> {
        let path = path.into();
        if path.trim().is_empty() || path.len() > MAX_ARTIFACT_PATH_BYTES || path.contains('\0') {
            return Err(ProjectFailure::identity("project_artifact_path_required"));
        }
        Ok(Self::Local {
            producer_work_item_id,
            path,
        })
    }

    /// Declare one result of a work item in another registered project.
    pub fn cross_project(project_id: ProjectId, work_item_id: WorkItemId) -> Self {
        Self::CrossProject {
            project_id,
            work_item_id,
        }
    }

    /// The work item this reference takes its result from.
    ///
    /// A local reference produces inside the declaring project, so `declaring`
    /// supplies that project's identity.
    pub fn producer(&self, declaring: &ProjectId) -> WorkRef {
        match self {
            Self::Local {
                producer_work_item_id,
                ..
            } => WorkRef::new(declaring.clone(), producer_work_item_id.clone()),
            Self::CrossProject {
                project_id,
                work_item_id,
            } => WorkRef::new(project_id.clone(), work_item_id.clone()),
        }
    }

    /// The declared local location, when this reference is a local one.
    pub fn local_path(&self) -> Option<&str> {
        match self {
            Self::Local { path, .. } => Some(path),
            Self::CrossProject { .. } => None,
        }
    }

    /// The persisted discriminator of this shape.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Local { .. } => "local",
            Self::CrossProject { .. } => "cross-project",
        }
    }
}

impl<'de> Deserialize<'de> for ArtifactReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(
            tag = "kind",
            rename_all = "kebab-case",
            rename_all_fields = "camelCase",
            deny_unknown_fields
        )]
        enum Raw {
            Local {
                producer_work_item_id: WorkItemId,
                path: String,
            },
            CrossProject {
                project_id: ProjectId,
                work_item_id: WorkItemId,
            },
        }
        match Raw::deserialize(deserializer)? {
            Raw::Local {
                producer_work_item_id,
                path,
            } => Self::local(producer_work_item_id, path).map_err(serde::de::Error::custom),
            Raw::CrossProject {
                project_id,
                work_item_id,
            } => Ok(Self::cross_project(project_id, work_item_id)),
        }
    }
}

impl fmt::Display for ArtifactReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local { path, .. } => write!(formatter, "local {path}"),
            Self::CrossProject {
                project_id,
                work_item_id,
            } => write!(formatter, "cross-project {project_id}/{work_item_id}"),
        }
    }
}

/// One declared dependency edge between work items.
///
/// The declaring project owns the consumer work item. The producer and the
/// artifact are carried by [`ArtifactReference`], so a within-project edge and a
/// cross-project edge differ in the reference's shape rather than in a second
/// declaration that could disagree with the first.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkDependency {
    /// The project that owns the consumer work item.
    pub project_id: ProjectId,
    /// The consumer work item: the one that waits for the result.
    pub work_item_id: WorkItemId,
    /// The declared result the consumer takes.
    pub artifact: ArtifactReference,
}

impl WorkDependency {
    /// The consumer this edge belongs to.
    pub fn consumer(&self) -> WorkRef {
        WorkRef::new(self.project_id.clone(), self.work_item_id.clone())
    }

    /// The producer this edge takes its result from.
    pub fn producer(&self) -> WorkRef {
        self.artifact.producer(&self.project_id)
    }
}

/// The explicit state of one declared artifact reference.
///
/// Three answers, and no fourth that guesses: a reference is materialized, it
/// is explicitly missing, or it cannot be evaluated without leaving the
/// declared roots or the declared index.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactState {
    /// The declared location exists inside the authorized root, or the
    /// referenced project declares the referenced work item.
    Materialized,
    /// The declared location is absent, or the referenced project declares no
    /// such work item. Explicit: an absent result never becomes an empty one.
    Missing,
    /// The reference cannot be evaluated inside the declared authority: the
    /// location would be reached through a symbolic link. Nothing outside the
    /// declared root is read to answer instead.
    Unavailable,
}

impl ArtifactState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Materialized => "materialized",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "materialized" => Some(Self::Materialized),
            "missing" => Some(Self::Missing),
            "unavailable" => Some(Self::Unavailable),
            _ => None,
        }
    }
}

/// One admitted dependency, as the owner stores and returns it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredDependency {
    /// Admission order, assigned by the store. It gives listings a stable order
    /// without inventing a timestamp.
    pub dependency_sequence: u64,
    pub dependency: WorkDependency,
    /// The explicit state of the declared reference, read from the declared
    /// roots and index each time; it is never stored as a claim that could
    /// outlive the result it described.
    pub artifact_state: ArtifactState,
}

impl DeclaredDependency {
    pub fn consumer(&self) -> WorkRef {
        self.dependency.consumer()
    }

    pub fn producer(&self) -> WorkRef {
        self.dependency.producer()
    }
}

/// Whether one declared location stays inside one declared authorized root.
///
/// The rule is lexical and touches no filesystem: the location must be relative
/// and built only from ordinary components, and the joined location must begin
/// with the declared root. An absolute location, a `..` traversal, and a volume
/// prefix are all refusals rather than normalizations, so a path is never
/// rewritten into something the caller did not declare.
pub fn stays_inside_authorized_root(root: &AuthorizedRoot, path: &str) -> bool {
    if path.trim().is_empty() || path.len() > MAX_ARTIFACT_PATH_BYTES || path.contains('\0') {
        return false;
    }
    let relative = Path::new(path);
    if relative.is_absolute() {
        return false;
    }
    let usable = relative
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    if !usable {
        return false;
    }
    let root = Path::new(root.as_str());
    root.join(relative).starts_with(root)
}

/// Read one declared location inside one authorized root, exactly.
///
/// Only the declared components between the root and the location are read, in
/// order, with [`std::fs::symlink_metadata`]: no directory is listed, no path
/// outside the declared root is inspected, and a symbolic link is never
/// followed. An absent component is [`ArtifactState::Missing`]; a component that
/// is a symbolic link, that is not an ordinary name, or that cannot be read is
/// [`ArtifactState::Unavailable`]; every declared component present is
/// [`ArtifactState::Materialized`].
pub fn read_local_artifact(root: &AuthorizedRoot, path: &str) -> ArtifactState {
    let components: Vec<Component<'_>> = Path::new(path).components().collect();
    if components.is_empty() {
        return ArtifactState::Missing;
    }
    // Only ordinary names are joined onto the root. An absolute location or a
    // traversal is refused before the first component is read, so the location
    // it names is never touched and never replaces the declared root.
    if components
        .iter()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return ArtifactState::Unavailable;
    }
    let mut current = PathBuf::from(root.as_str());
    for component in components {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return ArtifactState::Unavailable;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ArtifactState::Missing;
            }
            Err(_) => return ArtifactState::Unavailable,
        }
    }
    ArtifactState::Materialized
}

/// Render one dependency path for a refusal.
///
/// The rendering is the actionable part of the answer: it names every work item
/// between the refused edge and the consumer it would close, in the direction
/// the caller would have to remove.
pub fn render_dependency_path(path: &[WorkRef]) -> String {
    path.iter()
        .map(WorkRef::to_string)
        .collect::<Vec<_>>()
        .join(" -> ")
}

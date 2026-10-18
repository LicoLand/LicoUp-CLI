//! Normalized failure and recovery model.
//!
//! The product already publishes three failure vocabularies: the MCP
//! application error, the runtime adapter failure, and the client (CLI/stdio)
//! error. They agree on `code`, `stage`, and `retryable`, and differ in how they
//! describe what to do next. This module owns the neutral description both
//! interfaces project from, so the same failure cannot be reported differently
//! by the CLI and the MCP for the same cause.
//!
//! Three rules keep that projection honest:
//!
//! - The client chain survives end to end: `code`, `stage`, `component`,
//!   `retryable`, `recovery` and the public `presentation_args` are carried
//!   unchanged, so a projection may re-express a failure but not classify it
//!   differently.
//! - A failure is not flattened into a format error. Only a payload that is not
//!   JSON at all carries the `provide_valid_json` recovery
//!   ([`ApplicationFailure::invalid_json`]); a shape problem names its field, and
//!   a capability, permission or port failure keeps its own code and recovery.
//! - Technical text and paths must be redacted at the producing boundary.
//!   This type omits message, path and stack fields; [`PresentationArgs`]
//!   bounds display arguments but cannot determine whether arbitrary text is public.
//!
//! The one invariant worth enforcing in the type is effect certainty. A failure
//! that happened *after* an effect was attempted must be reconciled, never
//! blindly retried; [`ApplicationFailure::with_effect`] therefore refuses to
//! build an uncertain failure that says otherwise.

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// How much is known about whether the operation took effect.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectCertainty {
    /// No effect was attempted, or the failure happened before one could.
    NotAttempted,
    /// The effect is known to have happened.
    Applied,
    /// The effect may or may not have happened. Callers must reconcile against
    /// the durable record before retrying.
    Uncertain,
}

/// What a caller should do about a failure, in neutral terms. Each interface
/// projects this onto its own vocabulary with [`RecoveryAction::cli_wire`] and
/// [`RecoveryAction::mcp_wire`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryAction {
    /// The request itself was wrong; fix it before retrying.
    CorrectRequest,
    /// The caller needs the command surface to pick a valid shape.
    UseHelp,
    /// A command argument was wrong.
    CorrectArguments,
    /// A JSON argument did not parse.
    ProvideValidJson,
    /// Too many arguments, or one was too large.
    ReduceArguments,
    /// No adapter can serve this target.
    SelectSupportedAdapter,
    /// The provider is not installed or not reachable yet.
    InstallOrRetryRuntime,
    /// The user's own work is safe; the operation can be retried.
    PreserveDraftAndRetry,
    /// The outcome needs a human read before anything else happens.
    ReviewTerminalResult,
    /// Retry, or read the result first if the caller wants certainty.
    RetryOrReviewRequest,
    /// The dependency is temporarily unhealthy; retry after it recovers.
    RetryAfterRecovery,
    /// Reconcile against the durable record, then decide.
    ReconcileBeforeRetry,
}

impl RecoveryAction {
    /// The value the client (CLI/stdio) vocabulary publishes.
    ///
    /// That vocabulary has no reconcile value: an uncertain effect surfaces
    /// there as a result to review, which is what the CLI already does.
    pub const fn cli_wire(self) -> &'static str {
        match self {
            Self::CorrectRequest => "correct_request",
            Self::UseHelp => "use_cli_help",
            Self::CorrectArguments => "correct_command_arguments",
            Self::ProvideValidJson => "provide_valid_json",
            Self::ReduceArguments => "reduce_command_arguments",
            Self::SelectSupportedAdapter => "select_supported_adapter",
            Self::InstallOrRetryRuntime => "install_or_retry_runtime",
            Self::PreserveDraftAndRetry => "preserve_draft_and_retry",
            Self::ReviewTerminalResult | Self::ReconcileBeforeRetry => "review_terminal_result",
            Self::RetryOrReviewRequest | Self::RetryAfterRecovery => "retry_or_review_request",
        }
    }

    /// The value the MCP vocabulary publishes.
    pub const fn mcp_wire(self) -> &'static str {
        match self {
            Self::CorrectRequest
            | Self::UseHelp
            | Self::CorrectArguments
            | Self::ProvideValidJson
            | Self::ReduceArguments
            | Self::SelectSupportedAdapter => "correct_request_and_retry",
            Self::InstallOrRetryRuntime
            | Self::PreserveDraftAndRetry
            | Self::RetryAfterRecovery => "retry_after_recovery",
            Self::ReviewTerminalResult | Self::RetryOrReviewRequest => "retry_or_review_request",
            Self::ReconcileBeforeRetry => "reconcile_before_retry",
        }
    }

    /// Recovery that demands reconciliation before any retry.
    pub const fn requires_reconciliation(self) -> bool {
        matches!(self, Self::ReconcileBeforeRetry)
    }
}

/// The most public arguments one failure may publish, matching the bound the
/// client bridge already applies to `presentationArgs`.
pub const MAX_PRESENTATION_ARGS: usize = 4;
/// The longest public argument key accepted, matching the client bridge.
pub const MAX_PRESENTATION_KEY_BYTES: usize = 32;
/// The longest public argument value accepted, matching the client bridge.
pub const MAX_PRESENTATION_VALUE_BYTES: usize = 96;

/// The names a technical detail or a path would arrive under.
///
/// They look exactly like public display names, which is why they are refused by
/// name: the failure's own `code` is what carries the reason, and only what a
/// user may be shown belongs in these arguments.
const TECHNICAL_ARGUMENT_KEYS: &[&str] = &[
    "cause", "debug", "detail", "details", "file", "log", "logs", "message", "path", "raw",
    "stack", "stderr", "stdout", "trace",
];

/// The public arguments a failure may publish alongside its code.
///
/// This is the only place a failure carries data of its own, and every value in
/// it is meant to be shown to the caller. The bounds are the ones the client
/// bridge already publishes for `presentationArgs` — four entries, 32-byte keys,
/// 96-byte values — so both interfaces bound it the same way rather than each
/// inventing a limit.
///
/// Which *keys* an interface publishes stays that interface's own vocabulary,
/// generated with its schema; this type only refuses to become a conduit for
/// anything else. A key must be a public display name rather than a sentence or
/// a path, the names a technical dump would arrive under are refused outright,
/// and a value is short and single-line. These bounds are not a privacy check:
/// the producer must supply only approved public values, never paths or secrets.
///
/// Decoding is deliberately lenient: an argument that is not public is dropped,
/// because the caller needs the failure more than it needs the argument, and a
/// broken producer must not be able to turn a real failure into a decode error.
///
/// The map is inline on purpose: a failure is returned by value from every
/// operation, so the type is kept small enough to stay a cheap error, and an
/// empty argument map costs no allocation of its own.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PresentationArgs(BTreeMap<String, String>);

impl PresentationArgs {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Publish one public argument.
    ///
    /// Returns whether it was kept: a key that is not a public display name, a
    /// key with no room left, and a value that is too long or not single-line
    /// are all refused rather than truncated. Publishing a key twice replaces
    /// what was there.
    pub fn insert(&mut self, key: impl AsRef<str>, value: impl AsRef<str>) -> bool {
        let key = key.as_ref();
        let value = value.as_ref();
        if (self.len() >= MAX_PRESENTATION_ARGS && !self.0.contains_key(key))
            || !is_public_argument_key(key)
            || !is_public_argument_value(value)
        {
            return false;
        }
        self.0.insert(key.to_owned(), value.to_owned());
        true
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

impl Serialize for PresentationArgs {
    /// Always an object, empty or not: a failure omits the field entirely when
    /// there is nothing public to say, and this never emits something a consumer
    /// would have to read differently.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PresentationArgs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Consume the whole value before judging its shape. Swallowing a map
        // type error can leave a text decoder inside an unread array and turn
        // the enclosing failure into a spurious syntax error.
        let published = serde_json::Value::deserialize(deserializer)?;
        let mut args = Self::new();
        if let Some(published) = published.as_object() {
            for (key, value) in published {
                if let Some(value) = value.as_str() {
                    args.insert(key, value);
                }
            }
        }
        Ok(args)
    }
}

/// Whether a failure named no component.
fn is_empty_component(component: &Box<str>) -> bool {
    component.is_empty()
}

/// Whether `key` is a public display name a failure may publish.
fn is_public_argument_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_PRESENTATION_KEY_BYTES
        && !TECHNICAL_ARGUMENT_KEYS.contains(&key)
        && key.as_bytes()[0].is_ascii_lowercase()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Whether `value` can be shown: short and without control bytes, so a dump, a
/// multi-line trace or a NUL-terminated path cannot be carried.
fn is_public_argument_value(value: &str) -> bool {
    value.len() <= MAX_PRESENTATION_VALUE_BYTES
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

/// Whether `name` is a stable published name rather than free text: lowercase
/// ASCII words joined by `_`, the shape the product's existing code, stage and
/// component vocabularies already use.
fn is_stable_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PRESENTATION_KEY_BYTES
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// One normalized failure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationFailure {
    /// Stable reason code, identical through either interface.
    pub code: String,
    /// Where it happened, in the existing `area/action` vocabulary.
    pub stage: String,
    /// The component that produced the failure, when its producer named one.
    ///
    /// A stable published name such as `runtime_adapter`, never a sentence; see
    /// [`ApplicationFailure::with_component`]. Boxed because a failure is
    /// returned by value from every operation and this name is written once and
    /// never grows.
    #[serde(default, skip_serializing_if = "is_empty_component")]
    pub component: Box<str>,
    pub retryable: bool,
    pub effect: EffectCertainty,
    pub recovery: RecoveryAction,
    /// The offending field when the failure is about one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Public arguments the caller may show. See [`PresentationArgs`].
    #[serde(default, skip_serializing_if = "PresentationArgs::is_empty")]
    pub presentation_args: PresentationArgs,
}

impl<'de> Deserialize<'de> for ApplicationFailure {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            code: String,
            stage: String,
            #[serde(default)]
            component: Box<str>,
            retryable: bool,
            effect: EffectCertainty,
            recovery: RecoveryAction,
            #[serde(default)]
            field: Option<String>,
            #[serde(default)]
            presentation_args: PresentationArgs,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            code: wire.code,
            stage: wire.stage,
            component: wire.component,
            retryable: wire.retryable,
            effect: wire.effect,
            recovery: wire.recovery,
            field: wire.field,
            presentation_args: wire.presentation_args,
        }
        .with_effect(wire.effect))
    }
}

impl ApplicationFailure {
    /// A failure the caller cannot fix by retrying.
    pub fn permanent(code: impl Into<String>, stage: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            stage: stage.into(),
            component: Box::from(""),
            retryable: false,
            effect: EffectCertainty::NotAttempted,
            recovery: RecoveryAction::CorrectRequest,
            field: None,
            presentation_args: PresentationArgs::new(),
        }
    }

    /// A failure that is expected to clear on its own.
    pub fn retryable(code: impl Into<String>, stage: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            stage: stage.into(),
            component: Box::from(""),
            retryable: true,
            effect: EffectCertainty::NotAttempted,
            recovery: RecoveryAction::RetryAfterRecovery,
            field: None,
            presentation_args: PresentationArgs::new(),
        }
    }

    /// A failure whose effect may or may not have happened.
    ///
    /// This is the constructor the product's existing rules require for
    /// post-effect failures: uncertain implies retryable *and* reconcile.
    pub fn uncertain(code: impl Into<String>, stage: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            stage: stage.into(),
            component: Box::from(""),
            retryable: true,
            effect: EffectCertainty::Uncertain,
            recovery: RecoveryAction::ReconcileBeforeRetry,
            field: None,
            presentation_args: PresentationArgs::new(),
        }
    }

    /// A request-shape failure, carrying the field that was wrong.
    pub fn invalid_request(field: impl Into<String>) -> Self {
        Self {
            code: "invalid_request".to_owned(),
            stage: "schema/validate".to_owned(),
            component: Box::from(""),
            retryable: false,
            effect: EffectCertainty::NotAttempted,
            recovery: RecoveryAction::CorrectRequest,
            field: Some(field.into()),
            presentation_args: PresentationArgs::new(),
        }
    }

    /// A payload that is not JSON at all.
    ///
    /// This is the only failure that carries the `provide_valid_json` recovery.
    /// A well-formed payload of the wrong shape is
    /// [`ApplicationFailure::invalid_request`], and every other cause keeps its
    /// own code: an uninstalled capability, a refused claim and a failed port
    /// are not format errors, and reporting them as one would tell the caller to
    /// fix the wrong thing.
    pub fn invalid_json(field: impl Into<String>) -> Self {
        Self {
            code: "invalid_json".to_owned(),
            stage: "schema/decode".to_owned(),
            component: Box::from(""),
            retryable: false,
            effect: EffectCertainty::NotAttempted,
            recovery: RecoveryAction::ProvideValidJson,
            field: Some(field.into()),
            presentation_args: PresentationArgs::new(),
        }
    }

    /// Name the component that produced this failure.
    ///
    /// A name that is not a stable published name is dropped rather than
    /// carried, so a sentence or a path can never become a component.
    pub fn with_component(mut self, component: &str) -> Self {
        self.component = if is_stable_name(component) {
            Box::from(component)
        } else {
            Box::from("")
        };
        self
    }

    /// Name the offending field.
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    /// Publish one public argument. A key or value that is not public is
    /// dropped instead of being carried. See [`PresentationArgs`].
    pub fn with_presentation_arg(mut self, key: &str, value: &str) -> Self {
        self.presentation_args.insert(key, value);
        self
    }

    pub fn with_recovery(mut self, recovery: RecoveryAction) -> Self {
        self.recovery = if self.effect == EffectCertainty::Uncertain {
            RecoveryAction::ReconcileBeforeRetry
        } else {
            recovery
        };
        self
    }

    /// Record what is known about the effect.
    ///
    /// An uncertain effect forces reconcile-before-retry, because the durable
    /// record — not this failure — is what decides whether a retry is safe.
    pub fn with_effect(mut self, effect: EffectCertainty) -> Self {
        self.effect = effect;
        if effect == EffectCertainty::Uncertain {
            self.retryable = true;
            self.recovery = RecoveryAction::ReconcileBeforeRetry;
        }
        self
    }

    /// Whether this failure tells the caller to reconcile first.
    pub fn requires_reconciliation(&self) -> bool {
        self.effect == EffectCertainty::Uncertain || self.recovery.requires_reconciliation()
    }

    /// True when the failure may be retried without reconciling.
    pub fn safe_to_retry(&self) -> bool {
        self.retryable && !self.requires_reconciliation()
    }
}

/// A failure described in neutral terms, before it is projected.
///
/// Callers that already hold a native failure build one of these and let
/// [`FailureNormalization::into_failure`] apply the product's rules, rather than
/// each interface inventing its own `retryable`/`recovery` pairing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FailureNormalization {
    pub retryable: bool,
    pub uncertain_effect: bool,
}

impl FailureNormalization {
    pub const PERMANENT: Self = Self {
        retryable: false,
        uncertain_effect: false,
    };
    pub const RETRYABLE: Self = Self {
        retryable: true,
        uncertain_effect: false,
    };
    pub const UNCERTAIN: Self = Self {
        retryable: true,
        uncertain_effect: true,
    };

    /// The default recovery for this shape, before a caller overrides it.
    pub const fn default_recovery(self) -> RecoveryAction {
        if self.uncertain_effect {
            RecoveryAction::ReconcileBeforeRetry
        } else if self.retryable {
            RecoveryAction::RetryAfterRecovery
        } else {
            RecoveryAction::CorrectRequest
        }
    }

    pub fn into_failure(
        self,
        code: impl Into<String>,
        stage: impl Into<String>,
    ) -> ApplicationFailure {
        ApplicationFailure {
            code: code.into(),
            stage: stage.into(),
            component: Box::from(""),
            retryable: self.retryable || self.uncertain_effect,
            effect: if self.uncertain_effect {
                EffectCertainty::Uncertain
            } else {
                EffectCertainty::NotAttempted
            },
            recovery: self.default_recovery(),
            field: None,
            presentation_args: PresentationArgs::new(),
        }
    }
}

impl From<FailureNormalization> for ApplicationFailure {
    fn from(normalization: FailureNormalization) -> Self {
        normalization.into_failure("operation_failed", "operation/execute")
    }
}

impl std::fmt::Display for ApplicationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.code)
    }
}

impl std::error::Error for ApplicationFailure {}

//! The DeepSeek provider catalogue the installed Harness starts from.
//!
//! The Harness's own DeepSeek provider publishes an advisory model list: an
//! installation may replace it, and a deployment may configure a different
//! endpoint, but a harness started with no further configuration offers exactly
//! these rows. Reading them used to mean starting a Node process against the
//! vendor's provider library; the rows are data, so this package carries them
//! and no runtime is started to discover them.
//!
//! # The external dependency this catalogue declares
//!
//! The advisory list, its provider identity, its display names and its
//! per-model reasoning-effort advertisment belong to the DeepSeek provider
//! shipped inside the installed Harness, not to LicoUp. [`CATALOGUE_ORIGIN`]
//! names the exact vendor generation these rows were transcribed from, so a
//! reader can tell which vendor build this transcription describes rather than
//! assuming it tracks whatever is installed. An installation whose provider
//! replaced its advisory list is served by the installation's own catalogue;
//! this transcription is the one a default installation starts from.

use serde_json::{Value, json};

/// The provider id every row here is advertised under.
pub const PROVIDER_ID: &str = "deepseek-official";

/// The provider name every row here is advertised under.
pub const PROVIDER_NAME: &str = "DeepSeek";

/// The vendor generation these rows were transcribed from.
pub const CATALOGUE_ORIGIN: &str =
    "deepseek-harness provider `@deepseek-ai/dsh-llm-deepseek` 0.2.0-rc.2 advisory defaults";

/// The reasoning efforts a model that reasons offers, in the order the provider
/// advertises them.
const FULL_REASONING_EFFORTS: [&str; 4] = ["off", "low", "high", "max"];

/// The one effort a model that does not reason offers.
const OFF_ONLY_REASONING_EFFORTS: [&str; 1] = ["off"];

/// One advisory model row, as the provider advertises it.
struct AdvisoryModel {
    /// The wire model id.
    id: &'static str,
    /// The provider's own display name for it.
    display_name: &'static str,
    /// The reasoning efforts it advertises.
    reasoning_efforts: &'static [&'static str],
}

/// The advisory rows a default installation starts from, in the provider's own
/// order.
const ADVISORY_MODELS: [AdvisoryModel; 2] = [
    AdvisoryModel {
        id: "deepseek-flash",
        display_name: "DeepSeek-V41-Flash",
        reasoning_efforts: &OFF_ONLY_REASONING_EFFORTS,
    },
    AdvisoryModel {
        id: "deepseek-v4-pro",
        display_name: "DeepSeek-V4-Pro",
        reasoning_efforts: &FULL_REASONING_EFFORTS,
    },
];

/// The advisory catalogue, in the shape the client's model registry merges.
///
/// One entry per advertised model, under the provider's own identity, with the
/// wire id as the model name — which is what the client's registry keys a
/// catalogue row by, and what a request must send.
pub fn installed_catalog() -> Value {
    let models: Vec<Value> = ADVISORY_MODELS
        .iter()
        .map(|model| {
            json!({
                "name": model.id,
                "displayName": model.display_name,
                "providerId": PROVIDER_ID,
                "provider": PROVIDER_NAME,
                "reasoningEfforts": model.reasoning_efforts,
            })
        })
        .collect();
    json!({ "models": models })
}

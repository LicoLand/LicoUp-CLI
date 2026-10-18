//! The local-service readiness vocabulary every serve-family adapter shares.
//!
//! Some Agents are not driven over a pipe but over a small HTTP service the
//! Agent's own program runs (`serve`). Those Agents agree on one thing: the
//! answer a capability probe produces is *which models this endpoint currently
//! offers, and which one it is on by default*. That answer is not any one
//! Agent's protocol — the model selector, the provider that carries it and the
//! catalogue they form are the same shape for every serve-family Agent — so it
//! lives here rather than in the client or in one Agent's package.
//!
//! What an Agent's package owns is how *its* documents are read into this
//! shape, and that reader is the package's own. A package returns these values;
//! the client's serve engine adopts them into its own record. Nothing here
//! names an Agent, a vendor field or a wire format.

/// One model an Agent's local service is currently offering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServeModel {
    /// The provider that carries the model.
    pub provider_id: String,
    /// The model's own identifier inside that provider.
    pub model_id: String,
}

impl ServeModel {
    /// The `provider/model` selector both the client and the Agent's own
    /// message contract use to name this model.
    pub fn selector(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

/// Every model one service reports, with the one it is currently on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServeModelCatalog {
    /// The model the service is currently configured to use.
    pub current: ServeModel,
    /// Every model the service reports, in the order it reported them.
    pub models: Vec<ServeModel>,
}

impl ServeModelCatalog {
    /// Resolve one requested selector against this catalogue.
    ///
    /// An absent selector is the catalogue's own current model — that is an
    /// answer, not a missing one. A selector that names a model exactly, or a
    /// model id that matches exactly one entry, resolves to that entry. A
    /// selector that still carries a provider prefix resolves to the pair it
    /// names. Anything else is `None`, so an unavailable model is refused
    /// rather than silently replaced by the current one.
    pub fn resolve(&self, selector: Option<&str>) -> Option<ServeModel> {
        let selector = selector.map(str::trim).filter(|value| !value.is_empty());
        let Some(selector) = selector else {
            return Some(self.current.clone());
        };
        if let Some(exact) = self
            .models
            .iter()
            .find(|model| model.selector() == selector)
        {
            return Some(exact.clone());
        }
        let mut model_id_matches = self
            .models
            .iter()
            .filter(|model| model.model_id == selector)
            .cloned()
            .collect::<Vec<_>>();
        if model_id_matches.len() == 1 {
            return model_id_matches.pop();
        }
        if let Some(current_provider_match) = model_id_matches
            .into_iter()
            .find(|model| model.provider_id == self.current.provider_id)
        {
            return Some(current_provider_match);
        }
        if let Some((provider_id, model_id)) = selector.split_once('/')
            && !provider_id.is_empty()
            && !model_id.is_empty()
        {
            return Some(ServeModel {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
            });
        }
        None
    }
}

/// What one capability probe learned from an Agent's service.
///
/// A package produces this only when its own documents were fully understood:
/// a service that did not answer every probe document reports no readiness at
/// all rather than a partial one.
#[derive(Clone, Debug)]
pub struct ServeReadiness {
    /// The service's own reported version.
    pub version: String,
    /// The models it offers and the one it is on.
    pub catalog: ServeModelCatalog,
    /// The redacted health document the probe read, in the package's own shape.
    pub health: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> ServeModelCatalog {
        ServeModelCatalog {
            current: ServeModel {
                provider_id: "alpha".to_owned(),
                model_id: "one".to_owned(),
            },
            models: vec![
                ServeModel {
                    provider_id: "alpha".to_owned(),
                    model_id: "one".to_owned(),
                },
                ServeModel {
                    provider_id: "beta".to_owned(),
                    model_id: "one".to_owned(),
                },
            ],
        }
    }

    #[test]
    fn absent_selector_is_the_current_model() {
        assert_eq!(catalog().resolve(None).unwrap().selector(), "alpha/one");
        assert_eq!(
            catalog().resolve(Some("  ")).unwrap().selector(),
            "alpha/one"
        );
    }

    #[test]
    fn ambiguous_model_id_resolves_on_the_current_provider() {
        // `one` is offered by two providers, so the id alone is ambiguous and
        // the current provider decides — never an arbitrary first match.
        assert_eq!(
            catalog().resolve(Some("one")).unwrap().selector(),
            "alpha/one"
        );
    }

    #[test]
    fn exact_selector_and_prefixed_pair_resolve_and_the_rest_is_refused() {
        assert_eq!(
            catalog().resolve(Some("beta/one")).unwrap().selector(),
            "beta/one"
        );
        assert_eq!(
            catalog().resolve(Some("gamma/two")).unwrap().selector(),
            "gamma/two"
        );
        assert!(catalog().resolve(Some("gamma")).is_none());
    }

    #[test]
    fn selector_is_the_provider_prefixed_model() {
        assert_eq!(
            ServeModel {
                provider_id: "kilo".to_owned(),
                model_id: "kilo-auto/free".to_owned(),
            }
            .selector(),
            "kilo/kilo-auto/free"
        );
    }
}

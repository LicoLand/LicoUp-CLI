#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServeEndpoint {
    pub host: String,
    pub port: u16,
    pub attach_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServeModel {
    pub provider_id: String,
    pub model_id: String,
}

impl ServeModel {
    pub fn selector(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServeModelCatalog {
    pub current: ServeModel,
    pub models: Vec<ServeModel>,
}

impl ServeModelCatalog {
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
        if let Some((provider_id, model_id)) = selector.split_once('/') {
            if !provider_id.is_empty() && !model_id.is_empty() {
                return Some(ServeModel {
                    provider_id: provider_id.to_string(),
                    model_id: model_id.to_string(),
                });
            }
        }
        None
    }
}

#[derive(Clone, Debug)]
pub struct ServeReadiness {
    pub version: String,
    pub catalog: ServeModelCatalog,
    pub health: serde_json::Value,
}

pub struct ServeAttachment {
    pub endpoint: ServeEndpoint,
    pub catalog: ServeModelCatalog,
    pub _lease: super::turn_control::EndpointLease,
}

impl ServeEndpoint {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        let host = host.into();
        Self {
            attach_url: format!("http://{}:{}", host, port),
            host,
            port,
        }
    }
}

//! The provider boundaries the Gateway Runtime starts with.
//!
//! The runtime owns this format: it is the program that reads the configuration
//! and routes requests through it, so the shape of a fresh one is declared here
//! rather than in the client that writes the file. The client's service manager
//! calls this to materialize the same document the runtime would materialize for
//! itself, so a Gateway started by the app and a Gateway started by the login
//! item are the same Gateway.

use crate::model::llm_gateway::{
    CredentialStyle, GatewayConfig, GatewayProvider, UpstreamProtocol,
};
use licoup_vault::api_key::LlmApiKeyProvider;

/// The fixed provider boundaries of a Gateway that has never been configured.
///
/// Model inventory is fetched from each provider with the live authorized
/// credential and is never baked into this document.
pub fn default_config() -> GatewayConfig {
    let providers = vec![
        GatewayProvider {
            id: "kimi".to_owned(),
            base_url: "https://api.moonshot.cn/v1".to_owned(),
            protocol: UpstreamProtocol::OpenAiChatCompletions,
            credential_provider: LlmApiKeyProvider::Kimi,
            credential_style: CredentialStyle::Bearer,
        },
        GatewayProvider {
            id: "deepseek".to_owned(),
            base_url: "https://api.deepseek.com/v1".to_owned(),
            protocol: UpstreamProtocol::OpenAiChatCompletions,
            credential_provider: LlmApiKeyProvider::DeepSeek,
            credential_style: CredentialStyle::Bearer,
        },
        GatewayProvider {
            id: "kilo".to_owned(),
            base_url: "https://api.kilo.ai/api/gateway".to_owned(),
            protocol: UpstreamProtocol::OpenAiChatCompletions,
            credential_provider: LlmApiKeyProvider::Kilo,
            credential_style: CredentialStyle::Bearer,
        },
    ];
    GatewayConfig {
        schema_version: 1,
        providers,
        routes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::llm_gateway::CompiledGateway;

    #[test]
    fn the_default_provider_boundaries_compile() {
        let config = default_config();
        let text = serde_json::to_string_pretty(&config).unwrap();
        let parsed: GatewayConfig = serde_json::from_str(&text).unwrap();
        CompiledGateway::compile(parsed).unwrap();
        assert_eq!(config.providers.len(), LlmApiKeyProvider::ALL.len());
        assert!(config.routes.is_empty());
        assert!(text.contains("\"credentialProvider\": \"kimi\""));
        assert!(text.contains("\"credentialProvider\": \"kilo\""));
        assert!(!text.contains("requestedModel"));
        assert!(!text.contains("upstreamModel"));
        assert!(text.contains("\"baseUrl\": \"https://api.kilo.ai/api/gateway\""));
        assert!(text.contains("\"protocol\": \"open_ai_chat_completions\""));
    }
}

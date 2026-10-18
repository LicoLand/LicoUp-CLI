use super::super::endpoint::ServeEndpoint;
use super::super::endpoint::{ServeModel, ServeModelCatalog};

#[test]
fn endpoint_constructs_only_the_supplied_loopback_identity() {
    let endpoint = ServeEndpoint::new("127.0.0.1", 4097);
    assert_eq!(endpoint.host, "127.0.0.1");
    assert_eq!(endpoint.port, 4097);
    assert_eq!(endpoint.attach_url, "http://127.0.0.1:4097");
}

#[test]
fn model_resolution_preserves_nested_ids_and_uses_current_provider_for_bare_ids() {
    let catalog = ServeModelCatalog {
        current: ServeModel {
            provider_id: "kilo".to_string(),
            model_id: "kilo-auto/free".to_string(),
        },
        models: vec![
            ServeModel {
                provider_id: "kilo".to_string(),
                model_id: "kilo-auto/free".to_string(),
            },
            ServeModel {
                provider_id: "other".to_string(),
                model_id: "kilo-auto/free".to_string(),
            },
        ],
    };
    assert_eq!(
        catalog.resolve(None).unwrap().selector(),
        "kilo/kilo-auto/free"
    );
    assert_eq!(
        catalog.resolve(Some("kilo-auto/free")).unwrap().selector(),
        "kilo/kilo-auto/free"
    );
    assert_eq!(
        catalog
            .resolve(Some("other/kilo-auto/free"))
            .unwrap()
            .selector(),
        "other/kilo-auto/free"
    );
}

use super::*;

#[test]
fn launch_is_fixed_to_gateway_attach_and_excludes_private_values() {
    let cwd = absolute_test_cwd();
    let launch = LaunchSpec::for_gateway_attach("openclaw", cwd.as_path(), "ws://127.0.0.1:24189");
    assert_eq!(ATTACH_ARGS_PREFIX, &["acp", "--url"]);
    assert_eq!(launch.args, ["acp", "--url", "ws://127.0.0.1:24189"]);
    assert!(!launch.args.join(" ").contains("private"));
    assert_eq!(attach_mode(18_789), "vendor-default");
}

#[test]
fn explicit_gateway_config_is_resolved_without_raw_url_projection() {
    let endpoint =
        resolve_gateway_endpoint("unused", &json!({"gatewayWsUrl": "ws://127.0.0.1:24189"}))
            .unwrap();
    assert_eq!(endpoint.port, 24_189);
    assert_eq!(endpoint.ws_url, "ws://127.0.0.1:24189");
}

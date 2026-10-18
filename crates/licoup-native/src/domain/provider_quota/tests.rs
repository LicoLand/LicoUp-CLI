//! Focused provider-quota tests over synthetic inputs constructed in memory.

use super::command::{self, QuotaSource};
use super::contract::{
    ProviderQuotaSnapshot, QuotaFetchError, QuotaProvider, QuotaStatus, SNAPSHOT_COLLECTION,
    quota_capabilities,
};
use super::persistence::{client_state_store, load_retained};
use super::scheduler::{self, RefreshGate};
use super::{antigravity, codex, cursor, kimi_code, redaction};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;

const SYNTHETIC_NOW: &str = "2026-08-29T00:00:00Z";

fn synthetic_credential(provider: &str, role: &str) -> String {
    [provider, role, "synthetic"].join("-")
}

fn synthetic_jwt(claims: Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    let signature = ["synthetic", "signature"].concat();
    [header, payload, signature].join(".")
}

fn codex_usage_payload() -> Value {
    json!({
        "rate_limit": {
            "primary_window": {
                "used_percent": 42.5,
                "limit_window_seconds": 18_000,
                "reset_at": 2_000_000_000
            },
            "secondary_window": {
                "used_percent": 110.0,
                "limit_window_seconds": 604_800,
                "reset_at": 2_000_003_600
            }
        }
    })
}

fn cursor_usage_payload() -> Value {
    json!({
        "individualUsage": {
            "plan": {
                "totalPercentUsed": 42.5,
                "autoPercentUsed": 50.0,
                "apiPercentUsed": 10.0,
                "used": 850,
                "limit": 2000,
                "remaining": 1150
            },
            "onDemand": {
                "enabled": true,
                "used": 250,
                "limit": 5000,
                "remaining": 4750
            }
        },
        "billingCycleStart": "2033-05-01T00:00:00Z",
        "billingCycleEnd": "2033-06-01T00:00:00Z",
        "membershipType": "pro"
    })
}

fn kimi_usage_payload() -> Value {
    json!({
        "usage": {
            "limit": 100,
            "used": 31,
            "resetTime": "2033-06-01T00:00:00Z"
        },
        "limits": [{
            "window": {"duration": 5, "timeUnit": "TIME_UNIT_HOUR"},
            "detail": {
                "limit": 100,
                "used": 10,
                "resetTime": "2033-05-01T05:00:00Z"
            }
        }],
        "user": {"membership": {"level": "LEVEL_ADVANCED"}}
    })
}

fn kimi_string_counter_payload() -> Value {
    json!({
        "usage": {
            "limit": "200",
            "used": "50",
            "resetTime": "2033-06-01T00:00:00Z"
        },
        "user": {"membership": {"level": "LEVEL_FREE"}}
    })
}

fn active_cursor_token() -> String {
    synthetic_jwt(json!({
        "sub": "auth0|fixture-cursor-user",
        "email": "fixture@example.invalid",
        "exp": 2_000_000_000_i64
    }))
}

struct SyntheticKimiCredentials {
    document: String,
    access_token: String,
    refresh_token: String,
}

fn synthetic_kimi_credentials(expiry: i64) -> SyntheticKimiCredentials {
    let access_token = synthetic_jwt(json!({
        "sub": "fixture-kimi-user",
        "exp": expiry
    }));
    let refresh_token = synthetic_credential("kimi", "refresh");
    let document = json!({
        "access_token": access_token,
        "refresh_token": refresh_token,
        "expires_at": expiry
    })
    .to_string();
    SyntheticKimiCredentials {
        document,
        access_token,
        refresh_token,
    }
}

fn write_codex_auth(root: &Path) -> (PathBuf, String) {
    std::fs::create_dir_all(root).unwrap();
    let auth_path = root.join("auth.json");
    let access_token = synthetic_credential("codex", "access");
    let id_token = synthetic_jwt(json!({
        "email": "fixture@example.invalid",
        "chatgpt_plan_type": "plus"
    }));
    std::fs::write(
        &auth_path,
        json!({
            "tokens": {
                "access_token": access_token,
                "id_token": id_token
            }
        })
        .to_string(),
    )
    .unwrap();
    (auth_path, access_token)
}

fn fixed_now() -> OffsetDateTime {
    OffsetDateTime::parse(
        SYNTHETIC_NOW,
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
}

fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("lico-provider-quota-{label}-{nonce}"))
}

fn write_cursor_state_db(root: &Path, token: &str) -> PathBuf {
    std::fs::create_dir_all(root).unwrap();
    let db_path = root.join("state.vscdb");
    let connection = rusqlite::Connection::open(&db_path).unwrap();
    connection
        .execute(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO ItemTable (key, value) VALUES ('cursorAuth/accessToken', ?1)",
            [token],
        )
        .unwrap();
    db_path
}

fn codex_source_with_payload(
    auth_path: PathBuf,
    expected_token: String,
    payload: Value,
) -> codex::CodexSource {
    codex::CodexSource::for_testing(
        Some(auth_path),
        None,
        Box::new(move |url, bearer| {
            assert_eq!(url, "https://chatgpt.com/backend-api/wham/usage");
            assert_eq!(bearer, expected_token);
            Ok(payload.clone())
        }),
        Box::new(|_| panic!("app-server fallback must not run for the hosted lane")),
    )
}

#[test]
fn provider_quota_codex_source_normalizes_synthetic_hosted_payload() {
    let root = temp_root("codex-source");
    let (auth_path, access_token) = write_codex_auth(&root);
    let source = codex_source_with_payload(auth_path, access_token.clone(), codex_usage_payload());
    let snapshot = source.fetch_snapshot(fixed_now()).unwrap();

    assert_eq!(snapshot.agent_id, "codex");
    assert_eq!(snapshot.provider, QuotaProvider::Codex);
    assert_eq!(snapshot.status, QuotaStatus::Live);
    assert_eq!(snapshot.captured_at, SYNTHETIC_NOW);
    assert_eq!(snapshot.windows.len(), 2);
    let session = &snapshot.windows[0];
    assert_eq!(session.label, "session");
    assert_eq!(session.used_percent, 42.5);
    assert_eq!(session.window_minutes, Some(300));
    assert_eq!(session.resets_at.as_deref(), Some("2033-05-18T03:33:20Z"));
    let weekly = &snapshot.windows[1];
    assert_eq!(weekly.label, "weekly");
    // Raw provider values pass through unclamped, even above 100.
    assert_eq!(weekly.used_percent, 110.0);
    assert_eq!(weekly.window_minutes, Some(10080));
    assert_eq!(weekly.resets_at.as_deref(), Some("2033-05-18T04:33:20Z"));
    // Identity labels come from the local auth-store claims.
    assert_eq!(
        snapshot.identity.account_label.as_deref(),
        Some("fixture@example.invalid")
    );
    assert_eq!(snapshot.identity.plan.as_deref(), Some("plus"));

    let wire = snapshot.wire_value().to_string();
    assert!(!wire.contains(&access_token));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_codex_source_normalizes_hosted_reserve_window() {
    let root = temp_root("codex-hosted-reserve");
    let (auth_path, access_token) = write_codex_auth(&root);
    let payload = json!({
        "rate_limit": {
            "primary_window": {"used_percent": 100.0},
            "secondary_window": {"used_percent": 100.0}
        },
        "additional_rate_limits": [{
            "metered_feature": "base_model_inference",
            "limit_name": "gpt-reserve",
            "rate_limit": {"primary_window": {"used_percent": 48.0}}
        }]
    });
    let source = codex_source_with_payload(auth_path, access_token, payload);
    let snapshot = source.fetch_snapshot(fixed_now()).unwrap();

    assert_eq!(snapshot.windows.len(), 3);
    assert_eq!(snapshot.windows[2].label, "Luna Reserve");
    assert_eq!(snapshot.windows[2].used_percent, 48.0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_codex_source_falls_back_to_app_server_lane() {
    let payload = json!({
        "rateLimits": {
            "primary": {"usedPercent": 17.0, "windowMinutes": 300, "resetsAt": "2033-05-18T03:33:20Z"},
            "secondary": {"usedPercent": 3.0, "windowMinutes": 10080, "resetsAt": "2033-05-25T03:33:20Z"}
        }
    });
    let source = codex::CodexSource::for_testing(
        // No auth artifact: the hosted lane cannot run without a token.
        Some(temp_root("codex-fallback").join("absent-auth.json")),
        Some(PathBuf::from("fixture-codex-executable")),
        Box::new(|_, _| Err(QuotaFetchError::new("quota_endpoint_request_failed"))),
        Box::new(move |executable| {
            assert_eq!(executable, Path::new("fixture-codex-executable"));
            Ok(payload.clone())
        }),
    );
    let snapshot = source.fetch_snapshot(fixed_now()).unwrap();
    assert_eq!(snapshot.windows.len(), 2);
    assert_eq!(snapshot.windows[0].used_percent, 17.0);
    assert_eq!(snapshot.windows[0].window_minutes, Some(300));
    assert_eq!(
        snapshot.windows[0].resets_at.as_deref(),
        Some("2033-05-18T03:33:20Z")
    );
}

#[test]
fn provider_quota_codex_source_exposes_the_backend_reserve_window() {
    let payload = json!({
        "rateLimits": {
            "primary": {"usedPercent": 100.0, "windowMinutes": 300},
            "secondary": {"usedPercent": 100.0, "windowMinutes": 10080}
        },
        "rateLimitsByLimitId": {
            "base_model_inference": {
                "limitId": "base_model_inference",
                "limitName": "gpt-reserve",
                "primary": {"usedPercent": 48.0, "windowMinutes": 300}
            }
        }
    });
    let source = codex::CodexSource::for_testing(
        Some(temp_root("codex-reserve").join("absent-auth.json")),
        Some(PathBuf::from("fixture-codex-executable")),
        Box::new(|_, _| Err(QuotaFetchError::new("quota_endpoint_request_failed"))),
        Box::new(move |_| Ok(payload.clone())),
    );

    let snapshot = source.fetch_snapshot(fixed_now()).unwrap();
    assert_eq!(snapshot.windows.len(), 3);
    assert_eq!(snapshot.windows[0].label, "session");
    assert_eq!(snapshot.windows[1].label, "weekly");
    assert_eq!(snapshot.windows[2].label, "Luna Reserve");
    assert_eq!(snapshot.windows[2].used_percent, 48.0);
}

#[test]
fn provider_quota_codex_app_server_response_skips_notifications() {
    let messages = [
        json!({"jsonrpc": "2.0", "method": "account/updated", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 1, "result": {"ready": true}}),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "rateLimits": {
                    "primary": {"usedPercent": 17.0}
                }
            }
        }),
    ];
    let bytes = messages
        .iter()
        .map(|message| serde_json::to_string(message).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();
    let result = codex::parse_rate_limits_response(&bytes).unwrap();
    assert_eq!(
        result["rateLimits"]["primary"]["usedPercent"].as_f64(),
        Some(17.0)
    );
    assert!(codex::parse_rate_limits_response(b"{}").is_err());
    let rejected = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "error": {"code": -32602, "message": "invalid params"}
    }))
    .unwrap();
    let error = codex::parse_rate_limits_response(&rejected).unwrap_err();
    assert_eq!(error.code, "codex_app_server_capability_rejected");
}

#[test]
fn provider_quota_cursor_source_normalizes_synthetic_payload() {
    let root = temp_root("cursor-source");
    let token = active_cursor_token();
    let db_path = write_cursor_state_db(&root, &token);
    let expected_token = token.clone();
    let source = cursor::CursorSource::for_testing(
        Some(db_path),
        Box::new(move |url, cookie| {
            // cursor.com authenticates with the WorkOS session cookie derived
            // from the app token, never the raw token as a bearer credential.
            assert_eq!(
                cookie,
                format!("WorkosCursorSessionToken=fixture-cursor-user%3A%3A{expected_token}")
            );
            match url {
                "https://cursor.com/api/usage-summary" => Ok(cursor_usage_payload()),
                "https://cursor.com/api/auth/me" => Ok(json!({"email": "fixture@example.invalid"})),
                _ => panic!("unexpected cursor endpoint: {url}"),
            }
        }),
    );
    let snapshot = source.fetch_snapshot(fixed_now()).unwrap();

    assert_eq!(snapshot.agent_id, "cursor");
    assert_eq!(snapshot.provider, QuotaProvider::Cursor);
    assert_eq!(snapshot.status, QuotaStatus::Live);
    assert_eq!(snapshot.captured_at, SYNTHETIC_NOW);
    assert_eq!(snapshot.windows.len(), 4);
    let plan = &snapshot.windows[0];
    assert_eq!(plan.label, "plan");
    assert_eq!(plan.used_percent, 42.5);
    assert_eq!(plan.window_minutes, Some(44640));
    assert_eq!(plan.resets_at.as_deref(), Some("2033-06-01T00:00:00Z"));
    // Cursor meters plan and on-demand budgets in cents; the contract carries
    // USD so the card can show used/limit next to the percentage.
    assert_eq!(plan.used, Some(8.5));
    assert_eq!(plan.limit, Some(20.0));
    assert_eq!(plan.remaining, Some(11.5));
    assert_eq!(snapshot.windows[1].label, "auto");
    assert_eq!(snapshot.windows[1].used_percent, 50.0);
    assert_eq!(snapshot.windows[2].label, "api");
    assert_eq!(snapshot.windows[2].used_percent, 10.0);
    let on_demand = &snapshot.windows[3];
    assert_eq!(on_demand.label, "on-demand");
    assert_eq!(on_demand.used, Some(2.5));
    assert_eq!(on_demand.limit, Some(50.0));
    assert_eq!(on_demand.used_percent, 5.0);
    assert_eq!(snapshot.identity.plan.as_deref(), Some("pro"));
    assert_eq!(
        snapshot.identity.account_label.as_deref(),
        Some("fixture@example.invalid")
    );

    let wire = snapshot.wire_value().to_string();
    assert!(!wire.contains(&token));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_cursor_source_keeps_on_demand_distinct_from_plan() {
    let root = temp_root("cursor-on-demand");
    let token = active_cursor_token();
    let db_path = write_cursor_state_db(&root, &token);
    let payload = json!({
        "individualUsage": {
            "plan": {"totalPercentUsed": 12.0, "used": 240, "limit": 2000},
            // A disabled individual budget reports nothing; the shared team
            // budget is the account's real on-demand meter.
            "onDemand": {"enabled": false, "used": 0, "limit": null, "remaining": null}
        },
        "teamUsage": {
            "onDemand": {"enabled": true, "used": 1500, "limit": 10000, "remaining": 8500}
        },
        "billingCycleStart": "2033-05-01T00:00:00Z",
        "billingCycleEnd": "2033-06-01T00:00:00Z",
        "membershipType": "pro"
    });
    let source = cursor::CursorSource::for_testing(
        Some(db_path),
        Box::new(move |url, _| {
            if url == "https://cursor.com/api/usage-summary" {
                return Ok(payload.clone());
            }
            Ok(json!({}))
        }),
    );

    let snapshot = source.fetch_snapshot(fixed_now()).unwrap();
    let labels = snapshot
        .windows
        .iter()
        .map(|window| window.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(labels, ["plan", "on-demand"]);
    let on_demand = &snapshot.windows[1];
    assert_eq!(on_demand.used, Some(15.0));
    assert_eq!(on_demand.limit, Some(100.0));
    assert_eq!(on_demand.used_percent, 15.0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_cursor_source_rejects_token_without_user_id() {
    let root = temp_root("cursor-no-user-id");
    // Well-formed JWT whose payload carries an expiry but no `sub` claim.
    let token = synthetic_jwt(json!({
        "email": "fixture@example.invalid",
        "exp": 2_000_000_000_i64
    }));
    let db_path = write_cursor_state_db(&root, &token);
    let fetch_called = std::sync::Arc::new(AtomicBool::new(false));
    let fetch_flag = std::sync::Arc::clone(&fetch_called);
    let source = cursor::CursorSource::for_testing(
        Some(db_path),
        Box::new(move |_, _| {
            fetch_flag.store(true, Ordering::Relaxed);
            Err(QuotaFetchError::new("must not reach the network"))
        }),
    );
    let error = source.fetch_snapshot(fixed_now()).unwrap_err();
    assert_eq!(error.code, "cursor_auth_session_underivable");
    assert!(!fetch_called.load(Ordering::Relaxed));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_cursor_source_identity_fetch_is_best_effort() {
    let root = temp_root("cursor-identity-best-effort");
    let token = active_cursor_token();
    let db_path = write_cursor_state_db(&root, &token);
    let source = cursor::CursorSource::for_testing(
        Some(db_path),
        Box::new(move |url, _| {
            if url == "https://cursor.com/api/auth/me" {
                return Err(QuotaFetchError::new("identity endpoint down"));
            }
            Ok(cursor_usage_payload())
        }),
    );
    let snapshot = source.fetch_snapshot(fixed_now()).unwrap();
    // A failed identity fetch degrades to the JWT email claim, never to an
    // unavailable snapshot.
    assert_eq!(snapshot.status, QuotaStatus::Live);
    assert_eq!(
        snapshot.identity.account_label.as_deref(),
        Some("fixture@example.invalid")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_kimi_code_source_normalizes_synthetic_payload() {
    let root = temp_root("kimi-code-source");
    let credentials = synthetic_kimi_credentials(2_000_000_000);
    let credentials_dir = root.join("credentials");
    std::fs::create_dir_all(&credentials_dir).unwrap();
    let credentials_path = credentials_dir.join("kimi-code.json");
    std::fs::write(&credentials_path, &credentials.document).unwrap();
    let device_id_path = root.join("device_id");
    std::fs::write(&device_id_path, "fixture-device-id").unwrap();
    let expected_token = credentials.access_token.clone();
    let source = kimi_code::KimiCodeSource::for_testing(
        Some(credentials_path),
        Some(device_id_path),
        Box::new(move |url, headers| {
            assert_eq!(url, "https://api.kimi.com/coding/v1/usages");
            let owned: Vec<(String, String)> = headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect();
            assert_eq!(
                owned[0],
                (
                    "Authorization".to_string(),
                    format!("Bearer {expected_token}")
                )
            );
            assert!(owned.contains(&("X-Msh-Platform".to_string(), "kimi_code_cli".to_string())));
            assert!(owned.contains(&(
                "X-Msh-Device-Id".to_string(),
                "fixture-device-id".to_string()
            )));
            Ok(kimi_usage_payload())
        }),
    );
    let snapshot = source.fetch_snapshot(fixed_now()).unwrap();

    assert_eq!(snapshot.agent_id, "kimi-code");
    assert_eq!(snapshot.provider, QuotaProvider::KimiCode);
    assert_eq!(snapshot.status, QuotaStatus::Live);
    assert_eq!(snapshot.windows.len(), 2);
    let weekly = &snapshot.windows[0];
    assert_eq!(weekly.label, "weekly");
    assert_eq!(weekly.used_percent, 31.0);
    assert_eq!(weekly.window_minutes, Some(10080));
    assert_eq!(weekly.resets_at.as_deref(), Some("2033-06-01T00:00:00Z"));
    let session = &snapshot.windows[1];
    assert_eq!(session.label, "session");
    assert_eq!(session.used_percent, 10.0);
    assert_eq!(session.window_minutes, Some(300));
    assert_eq!(session.resets_at.as_deref(), Some("2033-05-01T05:00:00Z"));
    assert_eq!(snapshot.identity.plan.as_deref(), Some("Advanced"));
    assert_eq!(
        snapshot.identity.account_label.as_deref(),
        Some("fixture-kimi-user")
    );

    let wire = snapshot.wire_value().to_string();
    assert!(!wire.contains(&credentials.access_token));
    assert!(!wire.contains(&credentials.refresh_token));
    assert!(!wire.contains("fixture-device-id"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_kimi_code_source_accepts_string_counters() {
    let root = temp_root("kimi-code-string-counters");
    let credentials = synthetic_kimi_credentials(2_000_000_000);
    let credentials_dir = root.join("credentials");
    std::fs::create_dir_all(&credentials_dir).unwrap();
    let credentials_path = credentials_dir.join("kimi-code.json");
    std::fs::write(&credentials_path, credentials.document).unwrap();
    let source = kimi_code::KimiCodeSource::for_testing(
        Some(credentials_path),
        Some(root.join("device_id")),
        Box::new(move |_, _| Ok(kimi_string_counter_payload())),
    );
    let snapshot = source.fetch_snapshot(fixed_now()).unwrap();
    assert_eq!(snapshot.status, QuotaStatus::Live);
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(snapshot.windows[0].used_percent, 25.0);
    assert_eq!(snapshot.identity.plan.as_deref(), Some("Free"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_kimi_code_source_rejects_expired_token_without_fetching() {
    let root = temp_root("kimi-code-expired");
    let credentials_dir = root.join("credentials");
    std::fs::create_dir_all(&credentials_dir).unwrap();
    let credentials_path = credentials_dir.join("kimi-code.json");
    let credentials = synthetic_kimi_credentials(946_684_800);
    std::fs::write(&credentials_path, credentials.document).unwrap();
    let fetch_called = std::sync::Arc::new(AtomicBool::new(false));
    let fetch_flag = std::sync::Arc::clone(&fetch_called);
    let source = kimi_code::KimiCodeSource::for_testing(
        Some(credentials_path),
        None,
        Box::new(move |_, _| {
            fetch_flag.store(true, Ordering::Relaxed);
            Err(QuotaFetchError::new("must not reach the network"))
        }),
    );
    let error = source.fetch_snapshot(fixed_now()).unwrap_err();
    assert_eq!(error.code, "kimi_code_token_expired");
    assert!(!fetch_called.load(Ordering::Relaxed));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_kimi_code_source_missing_credentials_and_windows() {
    let root = temp_root("kimi-code-missing");
    let missing = kimi_code::KimiCodeSource::for_testing(
        Some(root.join("credentials").join("kimi-code.json")),
        None,
        Box::new(|_, _| unreachable!("no credentials, no fetch")),
    );
    let error = missing.fetch_snapshot(fixed_now()).unwrap_err();
    assert_eq!(error.code, "kimi_code_credentials_unreadable");

    let credentials = synthetic_kimi_credentials(2_000_000_000);
    let credentials_dir = root.join("credentials");
    std::fs::create_dir_all(&credentials_dir).unwrap();
    let credentials_path = credentials_dir.join("kimi-code.json");
    std::fs::write(&credentials_path, credentials.document).unwrap();
    let empty = kimi_code::KimiCodeSource::for_testing(
        Some(credentials_path),
        None,
        Box::new(|_, _| Ok(json!({"user": {}}))),
    );
    let error = empty.fetch_snapshot(fixed_now()).unwrap_err();
    assert_eq!(error.code, "kimi_code_quota_windows_missing");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_cursor_source_rejects_expired_token_without_fetching() {
    let root = temp_root("cursor-expired");
    let token = synthetic_jwt(json!({
        "sub": "auth0|fixture-cursor-user",
        "email": "fixture@example.invalid",
        "exp": 946_684_800_i64
    }));
    let db_path = write_cursor_state_db(&root, &token);
    let fetch_called = std::sync::Arc::new(AtomicBool::new(false));
    let fetch_flag = std::sync::Arc::clone(&fetch_called);
    let source = cursor::CursorSource::for_testing(
        Some(db_path),
        Box::new(move |_, _| {
            fetch_flag.store(true, Ordering::Relaxed);
            Err(QuotaFetchError::new("must not reach the network"))
        }),
    );
    let error = source.fetch_snapshot(fixed_now()).unwrap_err();
    assert_eq!(error.code, "cursor_auth_token_expired");
    assert!(!fetch_called.load(Ordering::Relaxed));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_antigravity_source_unavailable_without_binding() {
    let source = antigravity::AntigravitySource::for_testing(
        None,
        Box::new(|_| panic!("no request may run without a discovered binding")),
    );
    let error = source.fetch_snapshot(fixed_now()).unwrap_err();
    assert_eq!(error.code, "antigravity_loopback_lane_unavailable");
}

#[test]
fn provider_quota_scheduler_backoff_cadence_staleness_and_backfill() {
    let now = fixed_now();

    // Active vs idle cadence.
    assert_eq!(scheduler::cadence_seconds(true), 180);
    assert_eq!(scheduler::cadence_seconds(false), 1800);

    // Consecutive-failure backoff doubles from the base and caps.
    let first = scheduler::next_due_after_failure(now, 1);
    let second = scheduler::next_due_after_failure(now, 2);
    let capped = scheduler::next_due_after_failure(now, 32);
    let parse = |value: &str| {
        OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).unwrap()
    };
    assert_eq!((parse(&first) - now).whole_seconds(), 120);
    assert_eq!((parse(&second) - now).whole_seconds(), 240);
    assert_eq!(
        (parse(&capped) - now).whole_seconds(),
        scheduler::FAILURE_BACKOFF_CAP_SECONDS as i64
    );

    // Due selection.
    assert!(scheduler::is_due(None, now));
    assert!(scheduler::is_due(Some("2026-08-28T23:00:00Z"), now));
    assert!(!scheduler::is_due(Some("2026-08-29T00:03:00Z"), now));

    // Stale marking keeps the capture age visible through capturedAt.
    assert_eq!(
        scheduler::status_for("2026-08-29T00:00:00Z", 3600, now),
        QuotaStatus::Live
    );
    assert_eq!(
        scheduler::status_for("2026-08-28T21:00:00Z", 3600, now),
        QuotaStatus::Stale
    );

    // Missing reset timestamps backfill from the cached snapshot by label.
    let cached = vec![super::contract::QuotaWindow {
        label: "session".to_owned(),
        used_percent: 10.0,
        window_minutes: Some(300),
        resets_at: Some("2033-05-18T03:33:20Z".to_owned()),
        reset_description: String::new(),
        ..super::contract::QuotaWindow::default()
    }];
    let mut fresh = vec![super::contract::QuotaWindow {
        label: "session".to_owned(),
        used_percent: 11.0,
        window_minutes: Some(300),
        resets_at: None,
        reset_description: String::new(),
        ..super::contract::QuotaWindow::default()
    }];
    scheduler::backfill_reset_timestamps(&mut fresh, &cached);
    assert_eq!(fresh[0].resets_at.as_deref(), Some("2033-05-18T03:33:20Z"));

    // Single-flight coalescing guard.
    let gate = RefreshGate::new();
    let permit = gate.try_acquire().expect("first refresh acquires");
    assert!(gate.try_acquire().is_none(), "second refresh coalesces");
    drop(permit);
    assert!(
        gate.try_acquire().is_some(),
        "guard releases for the next tick"
    );
}

struct FixtureSource {
    snapshot: Option<ProviderQuotaSnapshot>,
}

impl QuotaSource for FixtureSource {
    fn fetch(&self, now: OffsetDateTime) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
        self.snapshot
            .clone()
            .map(|mut snapshot| {
                snapshot.captured_at = scheduler::format_rfc3339(now);
                snapshot
            })
            .ok_or_else(|| QuotaFetchError::new("fixture_fetch_failed"))
    }
}

fn synthetic_snapshot(provider: QuotaProvider, resets_at: Option<&str>) -> ProviderQuotaSnapshot {
    ProviderQuotaSnapshot {
        agent_id: provider.agent_id().to_owned(),
        provider,
        status: QuotaStatus::Live,
        windows: vec![super::contract::QuotaWindow {
            label: "session".to_owned(),
            used_percent: 42.5,
            window_minutes: Some(300),
            resets_at: resets_at.map(str::to_owned),
            reset_description: "5-hour window".to_owned(),
            ..super::contract::QuotaWindow::default()
        }],
        identity: Default::default(),
        captured_at: SYNTHETIC_NOW.to_owned(),
        stale_after_seconds: 3600,
    }
}

fn registry(entries: Vec<(QuotaProvider, ProviderQuotaSnapshot)>) -> command::SourceRegistry {
    entries
        .into_iter()
        .map(|(provider, snapshot)| {
            (
                provider,
                Box::new(FixtureSource {
                    snapshot: Some(snapshot),
                }) as Box<dyn QuotaSource>,
            )
        })
        .collect()
}

fn snapshot_wire_keys() -> [&'static str; 7] {
    [
        "agentId",
        "provider",
        "status",
        "windows",
        "identity",
        "capturedAt",
        "staleAfterSeconds",
    ]
}

#[test]
fn provider_quota_projection_emits_fixed_wire_shape() {
    let root = temp_root("wire-shape");
    let sources = registry(vec![
        (
            QuotaProvider::Codex,
            synthetic_snapshot(QuotaProvider::Codex, Some("2033-05-18T03:33:20Z")),
        ),
        (
            QuotaProvider::Cursor,
            synthetic_snapshot(QuotaProvider::Cursor, None),
        ),
    ]);
    let result = command::snapshot_with_sources(
        &json!({
            "stateRoot": root.to_string_lossy(),
            "now": SYNTHETIC_NOW,
            "forceRefresh": true
        }),
        &sources,
        &RefreshGate::new(),
    )
    .unwrap();

    assert_eq!(result["ok"], true);
    assert_eq!(result["schemaVersion"], "v0.0.1:provider-quota-snapshots-1");
    assert_eq!(result["generatedAt"], SYNTHETIC_NOW);
    let snapshots = result["snapshots"].as_array().unwrap();
    assert_eq!(snapshots.len(), 2);
    for snapshot in snapshots {
        let mut keys = snapshot
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        let mut expected = snapshot_wire_keys();
        expected.sort_unstable();
        assert_eq!(keys, expected);
    }
    let codex = snapshots
        .iter()
        .find(|snapshot| snapshot["provider"] == "codex")
        .unwrap();
    assert_eq!(codex["status"], "live");
    let window = &codex["windows"][0];
    assert_eq!(window["label"], "session");
    assert_eq!(window["usedPercent"], 42.5);
    assert_eq!(window["windowMinutes"], 300);
    assert_eq!(window["resetsAt"], "2033-05-18T03:33:20Z");
    assert_eq!(window["resetDescription"], "5-hour window");
    // Capability flags reach the projection from the packaged inventory.
    let capabilities = result["capabilities"].as_array().unwrap();
    assert!(
        capabilities
            .iter()
            .any(|entry| entry["agentId"] == "codex" && entry["provider"] == "codex")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_agents_without_a_source_are_absent_from_snapshots() {
    let root = temp_root("absent-source");
    let sources = registry(vec![(
        QuotaProvider::Codex,
        synthetic_snapshot(QuotaProvider::Codex, Some("2033-05-18T03:33:20Z")),
    )]);
    let result = command::snapshot_with_sources(
        &json!({
            "stateRoot": root.to_string_lossy(),
            "now": SYNTHETIC_NOW,
            "agent": "opencode",
            "forceRefresh": true
        }),
        &sources,
        &RefreshGate::new(),
    )
    .unwrap();
    let snapshots = result["snapshots"].as_array().unwrap();
    assert!(
        snapshots.is_empty(),
        "an agent with no quota source renders no entry and no placeholder"
    );

    let serialized = result.to_string();
    assert!(!serialized.contains("opencode"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_refresh_failure_retains_stale_snapshot_with_backoff() {
    let root = temp_root("stale-retention");
    let live_sources = registry(vec![(
        QuotaProvider::Codex,
        synthetic_snapshot(QuotaProvider::Codex, Some("2033-05-18T03:33:20Z")),
    )]);
    command::snapshot_with_sources(
        &json!({
            "stateRoot": root.to_string_lossy(),
            "now": "2026-08-28T21:00:00Z",
            "forceRefresh": true
        }),
        &live_sources,
        &RefreshGate::new(),
    )
    .unwrap();

    // Two hours later the fetch fails: backoff engages and the retained
    // snapshot is marked stale with its capture age still visible.
    let failing: command::SourceRegistry = [(
        QuotaProvider::Codex,
        Box::new(FixtureSource { snapshot: None }) as Box<dyn QuotaSource>,
    )]
    .into_iter()
    .collect();
    let result = command::snapshot_with_sources(
        &json!({
            "stateRoot": root.to_string_lossy(),
            "now": "2026-08-28T23:30:00Z",
            "forceRefresh": true,
            "clientActive": false
        }),
        &failing,
        &RefreshGate::new(),
    )
    .unwrap();
    let snapshot = &result["snapshots"][0];
    assert_eq!(snapshot["status"], "stale");
    assert_eq!(snapshot["capturedAt"], "2026-08-28T21:00:00Z");

    let store = client_state_store(&json!({"stateRoot": root.to_string_lossy()})).unwrap();
    let retained = load_retained(&store).unwrap();
    let state = retained.get(&QuotaProvider::Codex).unwrap();
    assert_eq!(state.consecutive_failures, 1);
    let next_due = OffsetDateTime::parse(
        state.next_due_at.as_deref().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let attempted = OffsetDateTime::parse(
        state.last_attempt_at.as_deref().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert_eq!((next_due - attempted).whole_seconds(), 120);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_refresh_backfills_missing_reset_from_cache() {
    let root = temp_root("reset-backfill");
    let with_reset = registry(vec![(
        QuotaProvider::Cursor,
        synthetic_snapshot(QuotaProvider::Cursor, Some("2033-06-01T00:00:00Z")),
    )]);
    command::snapshot_with_sources(
        &json!({
            "stateRoot": root.to_string_lossy(),
            "now": SYNTHETIC_NOW,
            "forceRefresh": true
        }),
        &with_reset,
        &RefreshGate::new(),
    )
    .unwrap();

    // The next fetch omits the reset timestamp; the cached value backfills.
    let without_reset = registry(vec![(
        QuotaProvider::Cursor,
        synthetic_snapshot(QuotaProvider::Cursor, None),
    )]);
    let result = command::snapshot_with_sources(
        &json!({
            "stateRoot": root.to_string_lossy(),
            "now": SYNTHETIC_NOW,
            "forceRefresh": true
        }),
        &without_reset,
        &RefreshGate::new(),
    )
    .unwrap();
    assert_eq!(
        result["snapshots"][0]["windows"][0]["resetsAt"],
        "2033-06-01T00:00:00Z"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_coalescing_guard_serves_retained_while_refresh_runs() {
    let root = temp_root("coalescing");
    let live_sources = registry(vec![(
        QuotaProvider::Antigravity,
        synthetic_snapshot(QuotaProvider::Antigravity, Some("2033-05-19T03:33:20Z")),
    )]);
    command::snapshot_with_sources(
        &json!({
            "stateRoot": root.to_string_lossy(),
            "now": SYNTHETIC_NOW,
            "forceRefresh": true
        }),
        &live_sources,
        &RefreshGate::new(),
    )
    .unwrap();

    let gate = RefreshGate::new();
    let permit = gate
        .try_acquire()
        .expect("test holds the in-flight refresh");
    let must_not_fetch: command::SourceRegistry = [(
        QuotaProvider::Antigravity,
        Box::new(FixtureSource { snapshot: None }) as Box<dyn QuotaSource>,
    )]
    .into_iter()
    .collect();
    let result = command::snapshot_with_sources(
        &json!({
            "stateRoot": root.to_string_lossy(),
            "now": SYNTHETIC_NOW,
            "forceRefresh": true
        }),
        &must_not_fetch,
        &gate,
    )
    .unwrap();
    drop(permit);

    // The coalesced tick served the retained live snapshot without fetching.
    assert_eq!(result["snapshots"][0]["status"], "live");
    assert_eq!(result["snapshots"][0]["windows"][0]["usedPercent"], 42.5);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_backoff_skips_refetch_until_due_without_a_snapshot() {
    use std::sync::atomic::AtomicUsize;

    struct CountingSource {
        calls: std::sync::Arc<AtomicUsize>,
    }

    impl QuotaSource for CountingSource {
        fn fetch(&self, _now: OffsetDateTime) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Err(QuotaFetchError::new("fixture_fetch_failed"))
        }
    }

    let root = temp_root("backoff-no-snapshot");
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let failing: command::SourceRegistry = [(
        QuotaProvider::Codex,
        Box::new(CountingSource {
            calls: std::sync::Arc::clone(&calls),
        }) as Box<dyn QuotaSource>,
    )]
    .into_iter()
    .collect();
    let gate = RefreshGate::new();

    // First tick fetches, fails, and records the consecutive-failure backoff;
    // the projection carries an explicit unavailable entry, never fake quota.
    let first = command::snapshot_with_sources(
        &json!({"stateRoot": root.to_string_lossy(), "now": SYNTHETIC_NOW}),
        &failing,
        &gate,
    )
    .unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(first["snapshots"][0]["status"], "unavailable");
    assert_eq!(
        first["snapshots"][0]["windows"].as_array().unwrap().len(),
        0
    );

    // One minute later the provider sits inside its 120-second backoff: the
    // tick serves the retained unavailable projection without refetching.
    let second = command::snapshot_with_sources(
        &json!({
            "stateRoot": root.to_string_lossy(),
            "now": "2026-08-29T00:01:00Z"
        }),
        &failing,
        &gate,
    )
    .unwrap();
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "backoff bounds fetch attempts before a first success lands"
    );
    assert_eq!(second["snapshots"][0]["status"], "unavailable");

    // Past the backoff window the provider is due again and refetches.
    let _ = command::snapshot_with_sources(
        &json!({
            "stateRoot": root.to_string_lossy(),
            "now": "2026-08-29T00:03:00Z"
        }),
        &failing,
        &gate,
    )
    .unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_redaction_strips_credential_shaped_material() {
    let mut value = json!({
        "agentId": "codex",
        "nested": {"accessToken": "fixture-secret", "ok": 1},
        "note": "Authorization: Bearer fixture-secret",
        "csrfToken": "fixture-secret"
    });
    assert!(redaction::contains_credential_material(&value));
    redaction::redact_outgoing(&mut value);
    assert!(!redaction::contains_credential_material(&value));
    let serialized = value.to_string();
    assert!(!serialized.contains("fixture-secret"));
    assert_eq!(value["nested"]["ok"], 1);
}

#[test]
fn provider_quota_emitted_artifacts_carry_no_credential_material() {
    let root = temp_root("privacy-boundary");
    let sources = registry(vec![(
        QuotaProvider::Codex,
        synthetic_snapshot(QuotaProvider::Codex, Some("2033-05-18T03:33:20Z")),
    )]);
    let params = json!({
        "stateRoot": root.to_string_lossy(),
        "now": SYNTHETIC_NOW,
        "forceRefresh": true
    });
    let result = command::snapshot_with_sources(&params, &sources, &RefreshGate::new()).unwrap();
    assert!(!redaction::contains_credential_material(&result));

    // The retained collection on disk carries only quota metrics and labels.
    let store = client_state_store(&params).unwrap();
    let retained = store.read_collection(SNAPSHOT_COLLECTION).unwrap();
    let serialized = serde_json::to_string(&retained).unwrap();
    for needle in [
        "codex-access-synthetic",
        "access_token",
        "refresh_token",
        "Bearer",
    ] {
        assert!(
            !serialized.contains(needle),
            "retained state must not carry credential material: {needle}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_quota_capability_flags_match_supported_providers() {
    let capabilities = quota_capabilities();
    assert_eq!(capabilities.len(), 4);
    for entry in &capabilities {
        assert!(QuotaProvider::parse(&entry.provider).is_some());
        assert_eq!(entry.agent_id, entry.provider);
    }
}

#[test]
fn provider_quota_module_header_credits_codexbar_without_vendored_code() {
    let facade = include_str!("../provider_quota.rs");
    assert!(facade.contains("CodexBar"));
    assert!(facade.contains("MIT"));
    assert!(facade.contains("Peter Steinberger"));

    // No vendored CodexBar code exists in the crate: nothing carries the
    // MIT license body that a copied CodexBar source file would include.
    let license_body_marker = ["Permission is hereby granted", ", free of charge"].concat();
    let crate_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![crate_src];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                assert!(
                    !text.contains(&license_body_marker),
                    "vendored license text found in {}",
                    path.display()
                );
            }
        }
    }
}

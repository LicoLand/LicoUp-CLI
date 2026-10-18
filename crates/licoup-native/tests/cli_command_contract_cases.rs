//! Frozen acceptance cases for native CLI command admission.
//!
//! Executable acceptance cases for the CLI command contract.

use anyhow::Error;
use licoup_native::ffi::commands::{
    AdmittedCommand, CliCommandError, CliCommandSchema, CliExecution, CommandCardinality,
    OptionArity, OptionConstraintKind, RequiredArgumentKind, admit_cli_command,
    cli_command_schemas, execute_cli,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const ADMISSION_STAGE: &str = "cli/admission";
const ADMISSION_COMPONENT: &str = "native_cli";
const MAX_CLI_ARGUMENT_COUNT: usize = 4_096;
const MAX_CLI_ARGUMENT_BYTES: usize = 2 * 1024 * 1024;
// The whole published registry, not a remembered number: every route the
// command table registers must appear here exactly once, so the count is
// re-derived from `build_command_table` whenever a route lands. The
// cross-device entry's five routes and the project family's routes are part of
// the registry and were missing from this authority before them.
const AUTHORITATIVE_ROUTE_COUNT: usize = 208;

#[derive(Clone, Debug)]
struct RouteAuthority {
    module: &'static str,
    handler: &'static str,
    path: &'static str,
    required: &'static [(&'static str, RequiredArgumentKind)],
    cardinality: CommandCardinality,
    options: Vec<OptionAuthority>,
    constraints: &'static [ConstraintAuthority],
}

#[derive(Clone, Copy, Debug)]
struct OptionAuthority {
    name: &'static str,
    arity: OptionArity,
    repeatable: bool,
    value_kind: RequiredArgumentKind,
    required: bool,
}

#[derive(Clone, Copy, Debug)]
struct ConstraintAuthority {
    kind: OptionConstraintKind,
    members: &'static [&'static str],
    condition_option: Option<&'static str>,
    condition_value: Option<&'static str>,
    required_option: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct ExpectedAdmission {
    code: &'static str,
    recovery: &'static str,
}

const MISSING_COMMAND: ExpectedAdmission = ExpectedAdmission {
    code: "cli_command_missing",
    recovery: "use_cli_help",
};
const UNKNOWN_COMMAND: ExpectedAdmission = ExpectedAdmission {
    code: "cli_command_unknown",
    recovery: "use_cli_help",
};
const UNSUPPORTED_OPERATION: ExpectedAdmission = ExpectedAdmission {
    code: "cli_operation_unsupported",
    recovery: "use_cli_help",
};
const MISSING_ARGUMENT: ExpectedAdmission = ExpectedAdmission {
    code: "cli_required_argument_missing",
    recovery: "correct_command_arguments",
};
const UNEXPECTED_ARGUMENT: ExpectedAdmission = ExpectedAdmission {
    code: "cli_argument_unexpected",
    recovery: "correct_command_arguments",
};
const UNKNOWN_OPTION: ExpectedAdmission = ExpectedAdmission {
    code: "cli_option_unknown",
    recovery: "correct_command_arguments",
};
const MISSING_OPTION: ExpectedAdmission = ExpectedAdmission {
    code: "cli_required_option_missing",
    recovery: "correct_command_arguments",
};
const MISSING_OPTION_VALUE: ExpectedAdmission = ExpectedAdmission {
    code: "cli_option_value_missing",
    recovery: "correct_command_arguments",
};
const DUPLICATE_OPTION: ExpectedAdmission = ExpectedAdmission {
    code: "cli_option_duplicate",
    recovery: "correct_command_arguments",
};
const OPTION_CONSTRAINT: ExpectedAdmission = ExpectedAdmission {
    code: "cli_option_constraint_violation",
    recovery: "correct_command_arguments",
};
const INVALID_JSON: ExpectedAdmission = ExpectedAdmission {
    code: "cli_json_invalid",
    recovery: "provide_valid_json",
};
const ARGUMENT_COUNT_EXCEEDED: ExpectedAdmission = ExpectedAdmission {
    code: "cli_argument_count_exceeded",
    recovery: "reduce_command_arguments",
};
const ARGUMENT_BYTES_EXCEEDED: ExpectedAdmission = ExpectedAdmission {
    code: "cli_argument_bytes_exceeded",
    recovery: "reduce_command_arguments",
};

#[test]
fn exact_help_is_the_only_successful_usage_dispatch() {
    for args in [["help"], ["--help"], ["-h"]] {
        assert_eq!(
            execute_cli(strings(args)).expect("an exact help route must succeed"),
            CliExecution::Usage,
            "exact help route {args:?} must be the Usage success"
        );
    }

    let cases = [
        ("empty", Vec::new(), MISSING_COMMAND),
        (
            "help with an extra positional",
            strings(["help", "private-extra-cli-oracle"]),
            UNEXPECTED_ARGUMENT,
        ),
        (
            "state get with an extra positional",
            strings(["state", "get", "settings", "private-extra-cli-oracle"]),
            UNEXPECTED_ARGUMENT,
        ),
    ];

    for (label, args, expected) in cases {
        let error = execute_cli(args).expect_err(label);
        assert_admission_error(
            &error,
            expected,
            &[
                "private-route-cli-oracle",
                "private-action-cli-oracle",
                "private-extra-cli-oracle",
            ],
        );
    }

    assert_private_failure_pair(
        "unknown top-level route",
        strings(["private-alpha-route-731"]),
        strings(["private-bravo-route-947"]),
        UNKNOWN_COMMAND,
        &["private-alpha", "route-731", "private-bravo", "route-947"],
    );
}

#[test]
fn gateway_client_token_requires_a_bounded_agent_selector() {
    let admitted = admit_cli_command(strings(["gateway", "client-token", "--agent", "codex"]))
        .expect("the private token helper route must be admitted");
    assert_eq!(admitted.source_module(), "gateway.rs");
    assert_eq!(admitted.handler_name(), "handle_client_token");
    assert_eq!(admitted.option_text("agent"), Some("codex"));
    let error = admit_cli_command(strings(["gateway", "client-token"]))
        .expect_err("the agent selector must be required");
    assert_admission_error(&error, MISSING_OPTION, &[]);
}

#[test]
fn readonly_registry_projection_exactly_matches_public_help_authority() {
    let expected = route_authorities();
    let projected = cli_command_schemas();
    assert_eq!(projected.len(), expected.len());
    let expected_by_path = expected
        .into_iter()
        .map(|route| (route.path, route))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        expected_by_path.len(),
        AUTHORITATIVE_ROUTE_COUNT,
        "public help authority must not contain duplicate routes"
    );
    let projected_by_path = projected
        .iter()
        .map(|schema| (schema.path().join(" "), schema))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        projected_by_path.len(),
        projected.len(),
        "runtime registry projection must not contain duplicate routes"
    );
    assert_eq!(
        projected_by_path
            .keys()
            .map(|path| path.as_str())
            .collect::<Vec<_>>(),
        expected_by_path.keys().copied().collect::<Vec<_>>(),
        "runtime registry must contain every and only public-help routes"
    );
    for (path, expected) in expected_by_path {
        assert_projected_schema(
            projected_by_path
                .get(path)
                .expect("projected route must exist"),
            &expected,
        );
    }
}

#[test]
fn command_discovery_exposes_every_admitted_route_and_generated_method_without_runtime_state() {
    use licoup_native::contracts::conversation_protocol::{
        CONVERSATION_PROTOCOL_METHODS, CONVERSATION_PROTOCOL_VERSION,
    };
    let CliExecution::Json(catalog) = execute_cli(strings(["commands"])).unwrap() else {
        panic!("command discovery must return a JSON catalog");
    };
    assert_eq!(catalog["schema"], "licoup.cli-catalog/v1");
    assert_eq!(catalog["rpc"]["protocol"], CONVERSATION_PROTOCOL_VERSION);
    assert_eq!(
        catalog["rpc"]["methods"],
        json!(CONVERSATION_PROTOCOL_METHODS.as_slice())
    );
    let commands = catalog["commands"].as_array().unwrap();
    assert_eq!(commands.len(), AUTHORITATIVE_ROUTE_COUNT);
    for schema in cli_command_schemas() {
        let command = commands
            .iter()
            .find(|command| command["path"] == json!(schema.path()))
            .unwrap();
        assert_eq!(
            command["requiredPositionals"],
            json!(schema.required_positionals())
        );
        assert_eq!(command["options"], json!(schema.options()));
        assert_eq!(command["constraints"], json!(schema.constraints()));
        assert!(!command.as_object().unwrap().contains_key("sourceModule"));
    }
    let help = licoup_native::ffi::commands::cli_command_help().join("\n");
    for route in route_authorities() {
        assert!(
            help.lines().any(|line| {
                line.strip_prefix("  licoup ").is_some_and(|line| {
                    line == route.path
                        || line
                            .strip_prefix(route.path)
                            .is_some_and(|tail| tail.starts_with(' '))
                })
            }),
            "registered route must be discoverable in help"
        );
    }
    assert!(help.contains("caller state is advisory and cannot authorize"));
}

#[test]
fn stdin_json_routes_freeze_value_json_admission() {
    for (path, required) in [
        ("mcp http preview", true),
        ("mcp http execute", true),
        ("agent conversation open", false),
        ("agent conversation send", false),
        ("agent conversation steer", false),
        ("agent conversation cancel", false),
        ("agent conversation cleanup", false),
        ("agent conversation capabilities", false),
        ("agent conversation stream", false),
        ("conversation execute", true),
        ("strategy execute", true),
    ] {
        let route = route_authorities()
            .into_iter()
            .find(|route| route.path == path)
            .unwrap_or_else(|| panic!("missing frozen --stdin-json route {path}"));
        let option = route
            .options
            .iter()
            .find(|option| option.name == "stdin-json")
            .unwrap_or_else(|| panic!("{path} must expose --stdin-json"));
        assert_eq!(option.arity, OptionArity::Value);
        assert_eq!(option.value_kind, RequiredArgumentKind::Json);
        assert_eq!(option.required, required);
        assert!(!option.repeatable);
    }
}

#[test]
fn every_authoritative_route_is_admitted_without_executing_its_handler() {
    let authority = route_authorities();
    assert_eq!(
        authority.len(),
        AUTHORITATIVE_ROUTE_COUNT,
        "the public presentation help must expand to the frozen route count"
    );

    for route in authority {
        let path = strings(route.path.split_ascii_whitespace());
        let required_values = route
            .required
            .iter()
            .map(|(name, kind)| match kind {
                RequiredArgumentKind::Text => format!("accepted-{name}"),
                RequiredArgumentKind::Json => {
                    format!(r#"{{"oracle":"accepted-{name}"}}"#)
                }
            })
            .collect::<Vec<_>>();
        let mut minimum = path.clone();
        minimum.extend(required_values.iter().cloned());

        let mut insufficient = minimum.clone();
        insufficient
            .pop()
            .expect("every authority route has at least one literal path token");
        let missing = if insufficient.is_empty() {
            MISSING_COMMAND
        } else {
            MISSING_ARGUMENT
        };
        let insufficient_error = admit_cli_command(insufficient)
            .expect_err("one token below the route minimum must fail admission");
        assert_admission_error(&insufficient_error, missing, &[]);

        let mut valid = minimum.clone();
        let mut present_options = route
            .options
            .iter()
            .filter(|option| option.required)
            .copied()
            .collect::<Vec<_>>();
        for constraint in route.constraints {
            if matches!(
                constraint.kind,
                OptionConstraintKind::OneOf | OptionConstraintKind::AtLeastOne
            ) {
                let seed = route
                    .options
                    .iter()
                    .find(|option| Some(option.name) == constraint.members.first().copied())
                    .expect("constraint member must name a documented option");
                if !present_options
                    .iter()
                    .any(|option| option.name == seed.name)
                {
                    present_options.push(*seed);
                }
            }
        }
        for option in &present_options {
            append_option(&mut valid, *option);
        }
        let admitted = admit_cli_command(valid.clone()).unwrap_or_else(|error| {
            panic!(
                "the route's documented minimum schema must be admitted for {}: {error}",
                route.path
            )
        });
        assert_admitted_route(&admitted, &route, &required_values, &present_options);

        for required in route.options.iter().filter(|option| option.required) {
            let mut omitted = minimum.clone();
            for option in present_options
                .iter()
                .filter(|option| option.name != required.name)
            {
                append_option(&mut omitted, *option);
            }
            assert_admission_error(
                &admit_cli_command(omitted)
                    .expect_err("every required option omission must fail admission"),
                MISSING_OPTION,
                &[],
            );
        }

        let mut unknown = valid.clone();
        unknown.push("--private-unknown-option".to_string());
        assert_admission_error(
            &admit_cli_command(unknown).expect_err("unknown options must fail admission"),
            UNKNOWN_OPTION,
            &["private-unknown-option"],
        );

        let mut positional = valid.clone();
        positional.push("private-extra-positional".to_string());
        assert_admission_error(
            &admit_cli_command(positional)
                .expect_err("undocumented trailing positionals must fail admission"),
            UNEXPECTED_ARGUMENT,
            &["private-extra-positional"],
        );

        if let Some(representative) = route.options.iter().find(|option| {
            !option.required
                && !present_options
                    .iter()
                    .any(|present| present.name == option.name)
                && !route.constraints.iter().any(|constraint| {
                    matches!(
                        constraint.kind,
                        OptionConstraintKind::OneOf | OptionConstraintKind::MutuallyExclusive
                    ) && constraint.members.contains(&option.name)
                        && present_options
                            .iter()
                            .any(|present| constraint.members.contains(&present.name))
                })
        }) {
            let mut with_option = valid.clone();
            append_option(&mut with_option, *representative);
            let admitted = admit_cli_command(with_option).unwrap_or_else(|error| {
                panic!(
                    "documented representative option for {} must be admitted: {error}",
                    route.path
                )
            });
            let mut present = present_options.clone();
            present.push(*representative);
            assert_admitted_route(&admitted, &route, &required_values, &present);
        }

        for option in &route.options {
            if option.arity == OptionArity::Value {
                let mut missing_value = minimum.clone();
                for required in route
                    .options
                    .iter()
                    .filter(|required| required.required && required.name != option.name)
                {
                    append_option(&mut missing_value, *required);
                }
                missing_value.push(format!("--{}", option.name));
                assert_admission_error(
                    &admit_cli_command(missing_value)
                        .expect_err("value options must reject a missing value"),
                    MISSING_OPTION_VALUE,
                    &[],
                );
            }

            if !option.repeatable {
                let mut duplicate = valid.clone();
                if !option.required {
                    append_option(&mut duplicate, *option);
                }
                append_option(&mut duplicate, *option);
                assert_admission_error(
                    &admit_cli_command(duplicate)
                        .expect_err("non-repeatable options must reject duplicates"),
                    DUPLICATE_OPTION,
                    &[],
                );
            }

            if option.value_kind == RequiredArgumentKind::Json {
                let mut malformed = minimum.clone();
                for required in route
                    .options
                    .iter()
                    .filter(|required| required.required && required.name != option.name)
                {
                    append_option(&mut malformed, *required);
                }
                malformed.extend([
                    format!("--{}", option.name),
                    r#"{"private":"malformed-option""#.to_string(),
                ]);
                assert_admission_error(
                    &admit_cli_command(malformed)
                        .expect_err("typed JSON options must reject malformed JSON"),
                    INVALID_JSON,
                    &["malformed-option"],
                );
            }
        }
        for constraint in route.constraints {
            let invalid =
                constraint_violation_args(&route, &minimum, &present_options, *constraint);
            assert_admission_error(
                &admit_cli_command(invalid)
                    .expect_err("each documented option relationship must fail closed"),
                OPTION_CONSTRAINT,
                &[],
            );
        }
    }
}

#[test]
fn command_argument_limits_have_exact_boundaries_and_count_precedes_bytes() {
    for canary in ["private-alpha-count-731", "private-bravo-count-947"] {
        let allowed = counted_unknown_route(MAX_CLI_ARGUMENT_COUNT, canary, false);
        let allowed_error =
            execute_cli(allowed).expect_err("exactly 4096 arguments must pass bounds admission");
        assert_admission_error(
            &allowed_error,
            UNKNOWN_COMMAND,
            &[canary, canary.split('-').next_back().unwrap_or_default()],
        );
    }
    let count_alpha = execute_cli(counted_unknown_route(
        MAX_CLI_ARGUMENT_COUNT + 1,
        "private-alpha-count-731",
        false,
    ))
    .expect_err("4097 arguments must exceed the count bound");
    let count_bravo = execute_cli(counted_unknown_route(
        MAX_CLI_ARGUMENT_COUNT + 1,
        "private-bravo-count-947",
        false,
    ))
    .expect_err("4097 arguments must exceed the count bound");
    assert_equivalent_private_errors(
        &count_alpha,
        &count_bravo,
        ARGUMENT_COUNT_EXCEEDED,
        &["private-alpha", "count-731", "private-bravo", "count-947"],
    );

    let byte_alpha = exact_byte_argument("private-alpha-byte-731", MAX_CLI_ARGUMENT_BYTES);
    let byte_bravo = exact_byte_argument("private-bravo-byte-947", MAX_CLI_ARGUMENT_BYTES);
    let allowed_alpha = execute_cli(vec![byte_alpha])
        .expect_err("an exact 2 MiB argument must pass byte admission");
    let allowed_bravo = execute_cli(vec![byte_bravo])
        .expect_err("an exact 2 MiB argument must pass byte admission");
    assert_equivalent_private_errors(
        &allowed_alpha,
        &allowed_bravo,
        UNKNOWN_COMMAND,
        &["private-alpha", "byte-731", "private-bravo", "byte-947"],
    );

    let utf8_exact_alpha =
        exact_multibyte_argument("private-alpha-utf8-731", MAX_CLI_ARGUMENT_BYTES);
    let utf8_exact_bravo =
        exact_multibyte_argument("private-bravo-utf8-947", MAX_CLI_ARGUMENT_BYTES);
    assert_eq!(utf8_exact_alpha.len(), MAX_CLI_ARGUMENT_BYTES);
    assert!(utf8_exact_alpha.chars().count() < utf8_exact_alpha.len());
    let utf8_allowed_alpha = admit_cli_command(vec![utf8_exact_alpha])
        .expect_err("exactly 2 MiB of multibyte UTF-8 must pass byte admission");
    let utf8_allowed_bravo = admit_cli_command(vec![utf8_exact_bravo])
        .expect_err("exactly 2 MiB of multibyte UTF-8 must pass byte admission");
    assert_equivalent_private_errors(
        &utf8_allowed_alpha,
        &utf8_allowed_bravo,
        UNKNOWN_COMMAND,
        &["private-alpha", "utf8-731", "private-bravo", "utf8-947"],
    );

    let utf8_oversized_alpha =
        exact_multibyte_argument("private-alpha-utf8-731", MAX_CLI_ARGUMENT_BYTES + 1);
    let utf8_oversized_bravo =
        exact_multibyte_argument("private-bravo-utf8-947", MAX_CLI_ARGUMENT_BYTES + 1);
    let utf8_error_alpha = admit_cli_command(vec![utf8_oversized_alpha])
        .expect_err("2 MiB + 1 byte of multibyte UTF-8 must fail byte admission");
    let utf8_error_bravo = admit_cli_command(vec![utf8_oversized_bravo])
        .expect_err("2 MiB + 1 byte of multibyte UTF-8 must fail byte admission");
    assert_equivalent_private_errors(
        &utf8_error_alpha,
        &utf8_error_bravo,
        ARGUMENT_BYTES_EXCEEDED,
        &["private-alpha", "utf8-731", "private-bravo", "utf8-947"],
    );

    for (label, alpha, bravo) in [
        (
            "oversized intermediate argument",
            vec![
                "private-alpha-route-731".to_string(),
                exact_byte_argument("private-alpha-middle-731", MAX_CLI_ARGUMENT_BYTES + 1),
                "tail".to_string(),
            ],
            vec![
                "private-bravo-route-947".to_string(),
                exact_byte_argument("private-bravo-middle-947", MAX_CLI_ARGUMENT_BYTES + 1),
                "tail".to_string(),
            ],
        ),
        (
            "oversized final argument",
            vec![
                "private-alpha-route-731".to_string(),
                "middle".to_string(),
                exact_byte_argument("private-alpha-final-731", MAX_CLI_ARGUMENT_BYTES + 1),
            ],
            vec![
                "private-bravo-route-947".to_string(),
                "middle".to_string(),
                exact_byte_argument("private-bravo-final-947", MAX_CLI_ARGUMENT_BYTES + 1),
            ],
        ),
    ] {
        let alpha_error = admit_cli_command(alpha).expect_err(label);
        let bravo_error = admit_cli_command(bravo).expect_err(label);
        assert_equivalent_private_errors(
            &alpha_error,
            &bravo_error,
            ARGUMENT_BYTES_EXCEEDED,
            &[
                "private-alpha",
                "middle-731",
                "final-731",
                "private-bravo",
                "middle-947",
                "final-947",
            ],
        );
    }

    let individually_bounded_alpha = vec![
        exact_byte_argument("private-alpha-total-a-731", MAX_CLI_ARGUMENT_BYTES),
        exact_byte_argument("private-alpha-total-b-731", MAX_CLI_ARGUMENT_BYTES),
    ];
    let individually_bounded_bravo = vec![
        exact_byte_argument("private-bravo-total-a-947", MAX_CLI_ARGUMENT_BYTES),
        exact_byte_argument("private-bravo-total-b-947", MAX_CLI_ARGUMENT_BYTES),
    ];
    let total_alpha = admit_cli_command(individually_bounded_alpha)
        .expect_err("aggregate size above 2 MiB must not become a total-byte rejection");
    let total_bravo = admit_cli_command(individually_bounded_bravo)
        .expect_err("aggregate size above 2 MiB must not become a total-byte rejection");
    assert_equivalent_private_errors(
        &total_alpha,
        &total_bravo,
        UNKNOWN_COMMAND,
        &[
            "private-alpha",
            "total-a-731",
            "total-b-731",
            "private-bravo",
            "total-a-947",
            "total-b-947",
        ],
    );

    let oversized_alpha = exact_byte_argument("private-alpha-byte-731", MAX_CLI_ARGUMENT_BYTES + 1);
    let oversized_bravo = exact_byte_argument("private-bravo-byte-947", MAX_CLI_ARGUMENT_BYTES + 1);
    let byte_error_alpha = execute_cli(vec![oversized_alpha])
        .expect_err("a 2 MiB + 1 byte argument must exceed the byte bound");
    let byte_error_bravo = execute_cli(vec![oversized_bravo])
        .expect_err("a 2 MiB + 1 byte argument must exceed the byte bound");
    assert_equivalent_private_errors(
        &byte_error_alpha,
        &byte_error_bravo,
        ARGUMENT_BYTES_EXCEEDED,
        &["private-alpha", "byte-731", "private-bravo", "byte-947"],
    );

    let priority_alpha = execute_cli(counted_unknown_route(
        MAX_CLI_ARGUMENT_COUNT + 1,
        "private-alpha-priority-731",
        true,
    ))
    .expect_err("combined overflow must fail");
    let priority_bravo = execute_cli(counted_unknown_route(
        MAX_CLI_ARGUMENT_COUNT + 1,
        "private-bravo-priority-947",
        true,
    ))
    .expect_err("combined overflow must fail");
    assert_equivalent_private_errors(
        &priority_alpha,
        &priority_bravo,
        ARGUMENT_COUNT_EXCEEDED,
        &[
            "private-alpha",
            "priority-731",
            "private-bravo",
            "priority-947",
        ],
    );
}

#[test]
fn every_direct_index_command_family_rejects_missing_route_tokens_without_panicking() {
    let cases: &[(&str, &[&str])] = &[
        ("state get collection", &["state", "get"]),
        ("state set collection and payload", &["state", "set"]),
        ("state set payload", &["state", "set", "settings"]),
        ("agent conversation operation", &["agent", "conversation"]),
        ("agent pairing operation", &["agents", "pair"]),
        ("target inspect target", &["targets", "inspect"]),
        ("OpenCode serve action", &["opencode-serve"]),
        ("snapshot restore identifier", &["snapshots", "restore"]),
        ("snapshot root action", &["snapshots", "root"]),
        ("snapshot profile action", &["snapshots", "profiles"]),
        ("snapshot archive action", &["snapshots", "archive"]),
        (
            "snapshot archive job action",
            &["snapshots", "archive", "jobs"],
        ),
        ("conversation action", &["conversations"]),
        ("mobile relay noun and action", &["mobile", "relay"]),
        ("mobile relay action", &["mobile", "relay", "config"]),
    ];

    for (label, args) in cases {
        let error = execute_cli(strings(args.iter().copied())).expect_err(label);
        assert_admission_error(&error, MISSING_ARGUMENT, &[]);
    }
}

#[test]
fn unsupported_dynamic_family_actions_are_typed_failures_instead_of_usage() {
    let cases: &[(&str, &[&str])] = &[
        ("agent pairing", &["agents", "pair"]),
        ("snapshot root", &["snapshots", "root"]),
        ("snapshot profiles", &["snapshots", "profiles"]),
        ("snapshot archive", &["snapshots", "archive"]),
        ("snapshot archive jobs", &["snapshots", "archive", "jobs"]),
        ("conversation", &["conversations"]),
        ("OpenCode serve", &["opencode-serve"]),
        ("mobile relay", &["mobile", "relay", "config"]),
    ];

    for (label, prefix) in cases {
        let mut alpha = strings(prefix.iter().copied());
        alpha.push("private-alpha-action-731".to_string());
        let mut bravo = strings(prefix.iter().copied());
        bravo.push("private-bravo-action-947".to_string());
        assert_private_failure_pair(
            label,
            alpha,
            bravo,
            UNSUPPORTED_OPERATION,
            &["private-alpha", "action-731", "private-bravo", "action-947"],
        );
    }
}

#[test]
fn real_binary_admission_failures_do_not_emit_private_arguments() {
    let cases = [
        (
            "unknown",
            strings(["private-alpha-binary-unknown-731"]),
            strings(["private-bravo-binary-unknown-947"]),
            "private-alpha-binary-unknown-731",
            "private-bravo-binary-unknown-947",
        ),
        (
            "unsupported operation",
            strings(["state", "private-alpha-binary-action-731"]),
            strings(["state", "private-bravo-binary-action-947"]),
            "private-alpha-binary-action-731",
            "private-bravo-binary-action-947",
        ),
        (
            "unexpected argument",
            strings(["state", "get", "settings", "private-alpha-binary-extra-731"]),
            strings(["state", "get", "settings", "private-bravo-binary-extra-947"]),
            "private-alpha-binary-extra-731",
            "private-bravo-binary-extra-947",
        ),
        (
            "malformed JSON",
            strings([
                "state",
                "set",
                "settings",
                r#"{"private":"private-alpha-binary-json-731""#,
            ]),
            strings([
                "state",
                "set",
                "settings",
                r#"{"private":"private-bravo-binary-json-947""#,
            ]),
            "private-alpha-binary-json-731",
            "private-bravo-binary-json-947",
        ),
        (
            "argument count overflow",
            counted_unknown_route(
                MAX_CLI_ARGUMENT_COUNT + 1,
                "private-alpha-binary-count-731",
                false,
            ),
            counted_unknown_route(
                MAX_CLI_ARGUMENT_COUNT + 1,
                "private-bravo-binary-count-947",
                false,
            ),
            "private-alpha-binary-count-731",
            "private-bravo-binary-count-947",
        ),
    ];

    for (label, alpha_args, bravo_args, alpha_canary, bravo_canary) in cases {
        let alpha = run_lico_client(&alpha_args);
        let bravo = run_lico_client(&bravo_args);
        assert!(
            !alpha.status.success() && !bravo.status.success(),
            "{label} must remain a real binary admission failure"
        );
        let alpha_fragment = alpha_canary
            .rsplit("binary-")
            .next()
            .unwrap_or(alpha_canary);
        let bravo_fragment = bravo_canary
            .rsplit("binary-")
            .next()
            .unwrap_or(bravo_canary);
        let forbidden = [alpha_canary, bravo_canary, alpha_fragment, bravo_fragment];
        assert_process_output_redacted(&alpha, &forbidden, label);
        assert_process_output_redacted(&bravo, &forbidden, label);
        assert_eq!(alpha.stdout, bravo.stdout, "{label} stdout must be stable");
        assert_eq!(alpha.stderr, bravo.stderr, "{label} stderr must be stable");
    }

    let byte_alpha_canary = "private-alpha-binary-byte-731";
    let byte_bravo_canary = "private-bravo-binary-byte-947";
    let byte_alpha = run_lico_client_rpc(vec![exact_byte_argument(
        byte_alpha_canary,
        MAX_CLI_ARGUMENT_BYTES + 1,
    )]);
    let byte_bravo = run_lico_client_rpc(vec![exact_byte_argument(
        byte_bravo_canary,
        MAX_CLI_ARGUMENT_BYTES + 1,
    )]);
    assert_process_output_redacted(
        &byte_alpha,
        &[byte_alpha_canary, byte_bravo_canary, "byte-731", "byte-947"],
        "argument byte overflow",
    );
    assert_process_output_redacted(
        &byte_bravo,
        &[byte_alpha_canary, byte_bravo_canary, "byte-731", "byte-947"],
        "argument byte overflow",
    );
    assert_eq!(byte_alpha.stdout, byte_bravo.stdout);
    assert_eq!(byte_alpha.stderr, byte_bravo.stderr);
    for output in [&byte_alpha, &byte_bravo] {
        let line = std::str::from_utf8(&output.stdout)
            .expect("RPC stdout must be UTF-8")
            .lines()
            .find(|line| !line.trim().is_empty())
            .expect("RPC must return one line-delimited response");
        let response: Value = serde_json::from_str(line).expect("RPC response must be JSON");
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], ARGUMENT_BYTES_EXCEEDED.code);
        assert_eq!(response["error"]["stage"], ADMISSION_STAGE);
        assert_eq!(response["error"]["component"], ADMISSION_COMPONENT);
        assert_eq!(response["error"]["retryable"], false);
        assert_eq!(
            response["error"]["recovery"],
            ARGUMENT_BYTES_EXCEEDED.recovery
        );
    }
}

#[test]
fn stdio_rpc_projects_every_admission_error_class_with_stable_metadata() {
    assert_rpc_admission_pair(
        "empty command",
        Vec::new(),
        Vec::new(),
        MISSING_COMMAND,
        &[],
    );
    assert_rpc_admission_pair(
        "missing required positional",
        strings(["state", "set", "private-alpha-rpc-missing-731"]),
        strings(["state", "set", "private-bravo-rpc-missing-947"]),
        MISSING_ARGUMENT,
        &[
            "private-alpha",
            "missing-731",
            "private-bravo",
            "missing-947",
        ],
    );
    assert_rpc_admission_pair(
        "missing required option",
        strings([
            "snapshots",
            "archive",
            "jobs",
            "create",
            "--selection-mode",
            "all",
            "--path",
            "private-alpha-rpc-required-731",
        ]),
        strings([
            "snapshots",
            "archive",
            "jobs",
            "create",
            "--selection-mode",
            "all",
            "--path",
            "private-bravo-rpc-required-947",
        ]),
        MISSING_OPTION,
        &[
            "private-alpha",
            "required-731",
            "private-bravo",
            "required-947",
        ],
    );
    assert_rpc_admission_pair(
        "unknown route",
        strings(["private-alpha-rpc-unknown-731"]),
        strings(["private-bravo-rpc-unknown-947"]),
        UNKNOWN_COMMAND,
        &[
            "private-alpha",
            "unknown-731",
            "private-bravo",
            "unknown-947",
        ],
    );
    assert_rpc_admission_pair(
        "unsupported operation",
        strings(["state", "private-alpha-rpc-action-731"]),
        strings(["state", "private-bravo-rpc-action-947"]),
        UNSUPPORTED_OPERATION,
        &["private-alpha", "action-731", "private-bravo", "action-947"],
    );
    assert_rpc_admission_pair(
        "unknown option",
        strings(["state", "get", "settings", "--private-alpha-rpc-option-731"]),
        strings(["state", "get", "settings", "--private-bravo-rpc-option-947"]),
        UNKNOWN_OPTION,
        &["private-alpha", "option-731", "private-bravo", "option-947"],
    );
    assert_rpc_admission_pair(
        "unexpected positional",
        strings(["state", "get", "settings", "private-alpha-rpc-extra-731"]),
        strings(["state", "get", "settings", "private-bravo-rpc-extra-947"]),
        UNEXPECTED_ARGUMENT,
        &["private-alpha", "extra-731", "private-bravo", "extra-947"],
    );
    assert_rpc_admission_pair(
        "missing option value",
        strings([
            "activity",
            "list",
            "--target",
            "private-alpha-rpc-value-731",
            "--limit",
        ]),
        strings([
            "activity",
            "list",
            "--target",
            "private-bravo-rpc-value-947",
            "--limit",
        ]),
        MISSING_OPTION_VALUE,
        &["private-alpha", "value-731", "private-bravo", "value-947"],
    );
    assert_rpc_admission_pair(
        "duplicate option",
        strings([
            "activity",
            "list",
            "--limit",
            "private-alpha-rpc-duplicate-731",
            "--limit",
            "private-alpha-rpc-duplicate-731",
        ]),
        strings([
            "activity",
            "list",
            "--limit",
            "private-bravo-rpc-duplicate-947",
            "--limit",
            "private-bravo-rpc-duplicate-947",
        ]),
        DUPLICATE_OPTION,
        &[
            "private-alpha",
            "duplicate-731",
            "private-bravo",
            "duplicate-947",
        ],
    );
    assert_rpc_admission_pair(
        "option constraint",
        strings([
            "skill",
            "usage",
            "report",
            "--days",
            "private-alpha-rpc-constraint-731",
            "--from",
            "private-alpha-rpc-from-731",
        ]),
        strings([
            "skill",
            "usage",
            "report",
            "--days",
            "private-bravo-rpc-constraint-947",
            "--from",
            "private-bravo-rpc-from-947",
        ]),
        OPTION_CONSTRAINT,
        &[
            "private-alpha",
            "constraint-731",
            "from-731",
            "private-bravo",
            "constraint-947",
            "from-947",
        ],
    );
    assert_rpc_admission_pair(
        "malformed JSON option",
        strings([
            "secure-mesh",
            "file",
            "route",
            "--manifest",
            r#"{"private":"private-alpha-rpc-json-731""#,
        ]),
        strings([
            "secure-mesh",
            "file",
            "route",
            "--manifest",
            r#"{"private":"private-bravo-rpc-json-947""#,
        ]),
        INVALID_JSON,
        &["private-alpha", "json-731", "private-bravo", "json-947"],
    );
    for route in [
        "mcp http preview",
        "mcp http execute",
        "agent conversation open",
        "agent conversation cleanup",
        "agent conversation capabilities",
    ] {
        let mut alpha = route
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        alpha.extend(strings([
            "--stdin-json",
            r#"{"private":"private-alpha-rpc-stdin-json-731""#,
        ]));
        let mut bravo = route
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        bravo.extend(strings([
            "--stdin-json",
            r#"{"private":"private-bravo-rpc-stdin-json-947""#,
        ]));
        assert_rpc_admission_pair(
            &format!("{route} malformed --stdin-json"),
            alpha,
            bravo,
            INVALID_JSON,
            &[
                "private-alpha",
                "stdin-json-731",
                "private-bravo",
                "stdin-json-947",
            ],
        );
    }
    let conversation_root = temporary_directory("cli-conversation-rpc-admission");
    for route in [
        "agent conversation send",
        "agent conversation steer",
        "agent conversation cancel",
        "agent conversation stream",
        "conversation execute",
    ] {
        let mut alpha = route
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        alpha.extend(strings([
            "--stdin-json",
            r#"{"private":"private-alpha-rpc-stdin-json-731""#,
        ]));
        let mut bravo = route
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        bravo.extend(strings([
            "--stdin-json",
            r#"{"private":"private-bravo-rpc-stdin-json-947""#,
        ]));
        assert_conversation_rpc_admission_pair(
            &format!("{route} malformed --stdin-json"),
            alpha,
            bravo,
            INVALID_JSON,
            &[
                "private-alpha",
                "stdin-json-731",
                "private-bravo",
                "stdin-json-947",
            ],
            &conversation_root,
        );
    }
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    let _ = fs::remove_dir_all(conversation_root);
    assert_rpc_admission_pair(
        "argument count overflow",
        counted_unknown_route(
            MAX_CLI_ARGUMENT_COUNT + 1,
            "private-alpha-rpc-count-731",
            false,
        ),
        counted_unknown_route(
            MAX_CLI_ARGUMENT_COUNT + 1,
            "private-bravo-rpc-count-947",
            false,
        ),
        ARGUMENT_COUNT_EXCEEDED,
        &["private-alpha", "count-731", "private-bravo", "count-947"],
    );
    assert_rpc_admission_pair(
        "argument byte overflow",
        vec![exact_byte_argument(
            "private-alpha-rpc-byte-731",
            MAX_CLI_ARGUMENT_BYTES + 1,
        )],
        vec![exact_byte_argument(
            "private-bravo-rpc-byte-947",
            MAX_CLI_ARGUMENT_BYTES + 1,
        )],
        ARGUMENT_BYTES_EXCEEDED,
        &["private-alpha", "byte-731", "private-bravo", "byte-947"],
    );
}

#[test]
fn state_write_admission_is_bounded_atomic_and_redacted() {
    let _serial = cli_environment_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let portable_root = temporary_directory("cli-command-admission");
    let _portable = PortableDataOverride::set(&portable_root);

    let initial = json!({
        "items": [{
            "key": "cli-admission-oracle",
            "value": "original"
        }]
    });
    let replacement = json!({
        "items": [{
            "key": "cli-admission-oracle",
            "value": "replacement"
        }]
    });

    let baseline = state_set(&initial);
    assert_eq!(state_get(), baseline, "valid state set/get must round-trip");

    let missing_payload = execute_cli(strings(["state", "set", "settings"]))
        .expect_err("state set without a payload must fail admission");
    assert_admission_error(&missing_payload, MISSING_ARGUMENT, &[]);
    assert_state_unchanged(&baseline, "missing state-set payload");

    let state_unknown_alpha = execute_cli(strings(["state", "private-alpha-action-731"]))
        .expect_err("unknown state action must fail admission");
    assert_admission_error(
        &state_unknown_alpha,
        UNSUPPORTED_OPERATION,
        &["private-alpha", "action-731"],
    );
    assert_state_unchanged(&baseline, "first unsupported state action");
    let state_unknown_bravo = execute_cli(strings(["state", "private-bravo-action-947"]))
        .expect_err("unknown state action must fail admission");
    assert_admission_error(
        &state_unknown_bravo,
        UNSUPPORTED_OPERATION,
        &["private-bravo", "action-947"],
    );
    assert_state_unchanged(&baseline, "second unsupported state action");
    assert_equivalent_private_errors(
        &state_unknown_alpha,
        &state_unknown_bravo,
        UNSUPPORTED_OPERATION,
        &["private-alpha", "action-731", "private-bravo", "action-947"],
    );

    let malformed_alpha = r#"{"items":[{"value":"private-alpha-malformed-731"}]"#.to_string();
    let malformed_bravo = r#"{"items":[{"value":"private-bravo-malformed-947"}]"#.to_string();
    let malformed_error_alpha = execute_cli(strings([
        "state",
        "set",
        "settings",
        malformed_alpha.as_str(),
    ]))
    .expect_err("malformed state JSON must fail admission");
    assert_admission_error(
        &malformed_error_alpha,
        INVALID_JSON,
        &["private-alpha", "malformed-731"],
    );
    assert_state_unchanged(&baseline, "first malformed JSON");
    let malformed_error_bravo = execute_cli(strings([
        "state",
        "set",
        "settings",
        malformed_bravo.as_str(),
    ]))
    .expect_err("malformed state JSON must fail admission");
    assert_admission_error(
        &malformed_error_bravo,
        INVALID_JSON,
        &["private-bravo", "malformed-947"],
    );
    assert_state_unchanged(&baseline, "second malformed JSON");
    assert_equivalent_private_errors(
        &malformed_error_alpha,
        &malformed_error_bravo,
        INVALID_JSON,
        &[
            "private-alpha",
            "malformed-731",
            "private-bravo",
            "malformed-947",
        ],
    );

    let oversized_alpha = oversized_json_payload("private-alpha-oversized-731");
    let oversized_bravo = oversized_json_payload("private-bravo-oversized-947");
    let oversized_error_alpha = execute_cli(strings([
        "state",
        "set",
        "settings",
        oversized_alpha.as_str(),
    ]))
    .expect_err("oversized state payload must fail admission");
    assert_admission_error(
        &oversized_error_alpha,
        ARGUMENT_BYTES_EXCEEDED,
        &["private-alpha", "oversized-731"],
    );
    assert_state_unchanged(&baseline, "first oversized payload");
    let oversized_error_bravo = execute_cli(strings([
        "state",
        "set",
        "settings",
        oversized_bravo.as_str(),
    ]))
    .expect_err("oversized state payload must fail admission");
    assert_admission_error(
        &oversized_error_bravo,
        ARGUMENT_BYTES_EXCEEDED,
        &["private-bravo", "oversized-947"],
    );
    assert_state_unchanged(&baseline, "second oversized payload");
    assert_equivalent_private_errors(
        &oversized_error_alpha,
        &oversized_error_bravo,
        ARGUMENT_BYTES_EXCEEDED,
        &[
            "private-alpha",
            "oversized-731",
            "private-bravo",
            "oversized-947",
        ],
    );

    let replacement_text = replacement.to_string();
    let count_error_alpha = execute_cli(state_count_overflow(
        &replacement_text,
        "private-alpha-count-731",
    ))
    .expect_err("excessive argument count must fail admission");
    assert_admission_error(
        &count_error_alpha,
        ARGUMENT_COUNT_EXCEEDED,
        &["private-alpha", "count-731"],
    );
    assert_state_unchanged(&baseline, "first argument-count overflow");
    let count_error_bravo = execute_cli(state_count_overflow(
        &replacement_text,
        "private-bravo-count-947",
    ))
    .expect_err("excessive argument count must fail admission");
    assert_admission_error(
        &count_error_bravo,
        ARGUMENT_COUNT_EXCEEDED,
        &["private-bravo", "count-947"],
    );
    assert_state_unchanged(&baseline, "second argument-count overflow");
    assert_equivalent_private_errors(
        &count_error_alpha,
        &count_error_bravo,
        ARGUMENT_COUNT_EXCEEDED,
        &["private-alpha", "count-731", "private-bravo", "count-947"],
    );

    let extra_error_alpha = execute_cli(strings([
        "state",
        "set",
        "settings",
        replacement_text.as_str(),
        "private-alpha-extra-731",
    ]))
    .expect_err("an unexpected state-set positional must fail admission");
    assert_admission_error(
        &extra_error_alpha,
        UNEXPECTED_ARGUMENT,
        &["private-alpha", "extra-731"],
    );
    assert_state_unchanged(&baseline, "first unexpected positional");
    let extra_error_bravo = execute_cli(strings([
        "state",
        "set",
        "settings",
        replacement_text.as_str(),
        "private-bravo-extra-947",
    ]))
    .expect_err("an unexpected state-set positional must fail admission");
    assert_admission_error(
        &extra_error_bravo,
        UNEXPECTED_ARGUMENT,
        &["private-bravo", "extra-947"],
    );
    assert_state_unchanged(&baseline, "second unexpected positional");
    assert_equivalent_private_errors(
        &extra_error_alpha,
        &extra_error_bravo,
        UNEXPECTED_ARGUMENT,
        &["private-alpha", "extra-731", "private-bravo", "extra-947"],
    );

    let replacement_document = state_set(&replacement);
    assert_ne!(
        replacement_document, baseline,
        "the valid replacement must be observably distinct"
    );
    assert_eq!(
        state_get(),
        replacement_document,
        "valid state set/get must remain successful after rejected requests"
    );
}

fn assert_admission_error(error: &Error, expected: ExpectedAdmission, forbidden_inputs: &[&str]) {
    let typed = error
        .downcast_ref::<CliCommandError>()
        .expect("CLI admission failures must be downcastable to CliCommandError");
    assert_eq!(typed.code(), expected.code);
    assert_eq!(typed.stage(), ADMISSION_STAGE);
    assert_eq!(typed.component(), ADMISSION_COMPONENT);
    assert!(!typed.retryable());
    assert_eq!(typed.recovery(), expected.recovery);

    // String inspection is deliberately limited to the privacy oracle. Semantic
    // classification above relies exclusively on typed fields.
    let public_display = error.to_string();
    let public_chain = format!("{error:#}");
    let public_debug = format!("{error:?}");
    for forbidden in forbidden_inputs {
        assert!(
            !public_display.contains(forbidden)
                && !public_chain.contains(forbidden)
                && !public_debug.contains(forbidden),
            "public CLI errors must not echo command paths or payloads"
        );
    }
}

fn assert_private_failure_pair(
    label: &str,
    alpha_args: Vec<String>,
    bravo_args: Vec<String>,
    expected: ExpectedAdmission,
    forbidden_inputs: &[&str],
) {
    let alpha = execute_cli(alpha_args).expect_err(label);
    let bravo = execute_cli(bravo_args).expect_err(label);
    assert_equivalent_private_errors(&alpha, &bravo, expected, forbidden_inputs);
}

fn assert_equivalent_private_errors(
    alpha: &Error,
    bravo: &Error,
    expected: ExpectedAdmission,
    forbidden_inputs: &[&str],
) {
    assert_admission_error(alpha, expected, forbidden_inputs);
    assert_admission_error(bravo, expected, forbidden_inputs);
    assert_eq!(
        public_error_surfaces(alpha),
        public_error_surfaces(bravo),
        "private inputs in the same error class must have identical public errors"
    );
}

fn public_error_surfaces(error: &Error) -> (String, String, String) {
    (
        error.to_string(),
        format!("{error:#}"),
        format!("{error:?}"),
    )
}

fn run_lico_client(args: &[String]) -> Output {
    let home = SyntheticCliHome::new(None);
    let output = home
        .command()
        .args(args)
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE")
        .output()
        .expect("the real licoup binary must be runnable");
    home.stop().expect("synthetic CLI host cleanup");
    output
}

#[test]
fn native_cli_starts_and_reuses_its_durable_host_without_flutter() {
    {
        let _serial = cli_environment_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let oracle = temporary_directory("licoup-conversation-owner-oracle");
        let _portable = PortableDataOverride::set(&oracle);
        let service =
            licoup_native::domain::client_conversation::ConversationService::open(&oracle)
                .expect("synthetic Conversation owner opens");
        service
            .execute(json!({"action": "conversation.list"}))
            .expect("synthetic Conversation owner lists before RPC projection");
    }
    let root = temporary_directory("native-cli-durable-host");
    let home = SyntheticCliHome::new(Some(&root));
    let run = |args: &[&str], body: Value| {
        let mut child = home
            .command()
            .args(args)
            .args(["--stdin-json", "true"])
            .env("LICOUP_HOME", &root)
            .env("LICOUP_CLIENT_PID", std::process::id().to_string())
            .env_remove("RUST_LOG")
            .env_remove("RUST_BACKTRACE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        serde_json::to_writer(child.stdin.take().unwrap(), &body).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "synthetic native CLI operation failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let first = run(
        &["conversation", "execute"],
        json!({"action": "conversation.list"}),
    );
    assert_eq!(
        first["ok"], true,
        "synthetic conversation response: {first}"
    );
    let response = run(
        &["rpc", "call", "client.conversation.execute"],
        json!({"action": "conversation.list"}),
    );
    assert_eq!(response["protocol"], "licoup.stdio.v1");
    assert_eq!(response["ok"], true);
    assert_eq!(response["result"], first);
    let created = run(
        &["conversation", "execute"],
        json!({
            "action": "conversation.create",
            "title": "Synthetic CLI acceptance",
            "owner": {"id": "human:synthetic", "kind": "human"},
            "members": [{"principal": {
                "id": "agent:synthetic", "kind": "agent", "agentId": "synthetic-agent",
            }}],
        }),
    );
    assert_eq!(created["ok"], true);
    let conversation = &created["result"];
    let owner_id = &conversation["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .find(|membership| membership["principal"]["id"] == "human:synthetic")
        .unwrap()["id"];
    let posted = run(
        &["conversation", "execute"],
        json!({
            "action": "conversation.message.post",
            "conversationId": conversation["id"],
            "authorMembershipId": owner_id,
            "content": "Synthetic local message",
        }),
    );
    assert_eq!(posted["ok"], true);
    // No Agent is addressed and no strategy is selected. The runtime-required
    // action succeeds without launching an Agent or using user content.
    let dispatched = run(
        &["conversation", "execute"],
        json!({
            "action": "conversation.dispatch.after-post",
            "conversationId": conversation["id"],
            "eventId": posted["result"]["event"]["id"],
        }),
    );
    assert_eq!(dispatched["ok"], true);
    assert_eq!(dispatched["result"]["turns"], json!([]));
    home.stop().expect("synthetic durable host cleanup");
    let _ = fs::remove_dir_all(root);
}

fn run_lico_client_rpc(args: Vec<String>) -> Output {
    let home = SyntheticCliHome::new(None);
    let mut child = home
        .command()
        .args(["rpc", "stdio"])
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the real licoup RPC subprocess must start");
    let request = json!({
        "protocol": "licoup.stdio.v1",
        "id": "cli-admission-byte-boundary",
        "workflowId": "cli-admission-byte-boundary",
        "method": "execute",
        "args": args,
    });
    child
        .stdin
        .take()
        .expect("RPC stdin must be piped")
        .write_all(format!("{request}\n").as_bytes())
        .expect("RPC request must be writable");
    let output = child
        .wait_with_output()
        .expect("the real licoup RPC subprocess must finish");
    home.stop().expect("synthetic RPC host cleanup");
    output
}

fn run_lico_client_conversation_rpc(args: Vec<String>, portable_root: &Path) -> Output {
    let home = SyntheticCliHome::new(Some(portable_root));
    let mut child = home
        .command()
        .args(["rpc", "conversation"])
        .env("LICOUP_HOME", portable_root)
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the real persistent conversation RPC subprocess must start");
    let request = json!({
        "protocol": "licoup.stdio.v1",
        "id": "cli-conversation-admission",
        "workflowId": "cli-conversation-admission",
        "method": "execute",
        "args": args,
    });
    child
        .stdin
        .take()
        .expect("RPC stdin must be piped")
        .write_all(format!("{request}\n").as_bytes())
        .expect("RPC request must be writable");
    let output = child
        .wait_with_output()
        .expect("the real persistent conversation RPC subprocess must finish");
    home.stop().expect("synthetic conversation host cleanup");
    output
}

#[test]
fn persistent_conversation_rpc_accepts_a_request_after_its_first_response() {
    let portable_root = temporary_directory("licoup-conversation-rpc-sequential");
    let home = SyntheticCliHome::new(Some(&portable_root));
    let mut child = home
        .command()
        .args(["rpc", "conversation"])
        .env("LICOUP_HOME", &portable_root)
        .env_remove("LICOUP_CLIENT_PID")
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the persistent conversation RPC subprocess must start");
    let mut input = child.stdin.take().expect("RPC stdin must be piped");
    let mut output = BufReader::new(child.stdout.take().expect("RPC stdout must be piped"));

    for index in 1..=2 {
        let request_id = format!("sequential-{index}");
        let request = json!({
            "protocol": "licoup.stdio.v1",
            "id": request_id,
            "workflowId": "cli-conversation-sequential",
            "method": "client.conversation.execute",
            "params": {
                "action": "conversation.list",
                "includeArchived": false,
            },
        });
        input
            .write_all(format!("{request}\n").as_bytes())
            .expect("each RPC request must remain writable");
        input.flush().expect("each RPC request must be flushed");

        let mut line = String::new();
        assert_ne!(
            output
                .read_line(&mut line)
                .expect("RPC response must be readable"),
            0,
            "the proxy must remain connected for sequential response {index}"
        );
        let response: Value = serde_json::from_str(&line).expect("RPC response must be JSON");
        assert_eq!(response["id"], request_id);
        assert_eq!(
            response["ok"], true,
            "synthetic conversation response: {response}"
        );
    }

    drop(input);
    assert!(
        child
            .wait()
            .expect("the persistent conversation RPC subprocess must finish")
            .success()
    );
    home.stop().expect("synthetic sequential host cleanup");
    let _ = fs::remove_dir_all(portable_root);
}

fn rpc_response(output: &Output) -> Value {
    let line = std::str::from_utf8(&output.stdout)
        .expect("RPC stdout must be UTF-8")
        .lines()
        .find(|line| !line.trim().is_empty())
        .expect("RPC must return one line-delimited response");
    serde_json::from_str(line).expect("RPC response must be JSON")
}

fn assert_rpc_admission_pair(
    label: &str,
    alpha_args: Vec<String>,
    bravo_args: Vec<String>,
    expected: ExpectedAdmission,
    forbidden: &[&str],
) {
    let alpha = run_lico_client_rpc(alpha_args);
    let bravo = run_lico_client_rpc(bravo_args);
    assert_eq!(alpha.stdout, bravo.stdout, "{label} stdout must be stable");
    assert_eq!(alpha.stderr, bravo.stderr, "{label} stderr must be stable");
    assert_process_output_redacted(&alpha, forbidden, label);
    assert_process_output_redacted(&bravo, forbidden, label);
    for response in [rpc_response(&alpha), rpc_response(&bravo)] {
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], expected.code);
        assert_eq!(response["error"]["stage"], ADMISSION_STAGE);
        assert_eq!(response["error"]["component"], ADMISSION_COMPONENT);
        assert_eq!(response["error"]["retryable"], false);
        assert_eq!(response["error"]["recovery"], expected.recovery);
    }
}

fn assert_conversation_rpc_admission_pair(
    label: &str,
    alpha_args: Vec<String>,
    bravo_args: Vec<String>,
    expected: ExpectedAdmission,
    forbidden: &[&str],
    portable_root: &Path,
) {
    let alpha = run_lico_client_conversation_rpc(alpha_args, portable_root);
    let bravo = run_lico_client_conversation_rpc(bravo_args, portable_root);
    assert_eq!(alpha.stdout, bravo.stdout, "{label} stdout must be stable");
    assert_eq!(alpha.stderr, bravo.stderr, "{label} stderr must be stable");
    assert_process_output_redacted(&alpha, forbidden, label);
    assert_process_output_redacted(&bravo, forbidden, label);
    for response in [rpc_response(&alpha), rpc_response(&bravo)] {
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], expected.code);
        assert_eq!(response["error"]["stage"], ADMISSION_STAGE);
        assert_eq!(response["error"]["component"], ADMISSION_COMPONENT);
        assert_eq!(response["error"]["retryable"], false);
        assert_eq!(response["error"]["recovery"], expected.recovery);
    }
}

fn assert_process_output_redacted(output: &Output, forbidden: &[&str], label: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for canary in forbidden {
        assert!(
            !stdout.contains(canary) && !stderr.contains(canary),
            "{label} must not expose either request canary on stdout or stderr"
        );
    }
}

fn assert_projected_schema(actual: &CliCommandSchema, expected: &RouteAuthority) {
    assert_eq!(actual.source_module(), expected.module);
    assert_eq!(actual.handler_name(), expected.handler);
    assert_eq!(
        actual.path(),
        expected.path.split_ascii_whitespace().collect::<Vec<_>>()
    );
    assert_eq!(
        actual.cardinality(),
        expected.cardinality,
        "cardinality mismatch for {}",
        expected.path
    );
    assert_eq!(actual.required_positionals().len(), expected.required.len());
    for (actual, (name, kind)) in actual.required_positionals().iter().zip(expected.required) {
        assert_eq!(actual.name(), *name);
        assert_eq!(actual.kind(), *kind);
    }
    assert_eq!(
        actual.options().len(),
        expected.options.len(),
        "option count mismatch for {}",
        expected.path
    );
    for (actual, expected) in actual.options().iter().zip(&expected.options) {
        assert_eq!(actual.name(), expected.name);
        assert_eq!(actual.arity(), expected.arity);
        assert_eq!(actual.repeatable(), expected.repeatable);
        assert_eq!(actual.value_kind(), expected.value_kind);
        assert_eq!(actual.required(), expected.required);
    }
    assert_eq!(actual.constraints().len(), expected.constraints.len());
    for (actual, expected) in actual.constraints().iter().zip(expected.constraints) {
        assert_eq!(actual.kind(), expected.kind);
        assert_eq!(actual.members(), expected.members);
        assert_eq!(actual.condition_option(), expected.condition_option);
        assert_eq!(actual.condition_value(), expected.condition_value);
        assert_eq!(actual.required_option(), expected.required_option);
    }
}

fn assert_admitted_route(
    admitted: &AdmittedCommand,
    expected: &RouteAuthority,
    required_values: &[String],
    present_options: &[OptionAuthority],
) {
    let expected_path = expected.path.split_ascii_whitespace().collect::<Vec<_>>();
    let expected_required = expected
        .required
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    assert_eq!(admitted.source_module(), expected.module);
    assert_eq!(admitted.handler_name(), expected.handler);
    assert_eq!(admitted.path(), expected_path);
    assert_eq!(admitted.required_positionals(), expected_required);
    assert_eq!(admitted.cardinality(), expected.cardinality);
    assert_eq!(
        admitted.option_specs().len(),
        expected.options.len(),
        "admitted option count mismatch for {}",
        expected.path
    );
    for (actual, option) in admitted.option_specs().iter().zip(&expected.options) {
        assert_eq!(actual.name(), option.name);
        assert_eq!(actual.arity(), option.arity);
        assert_eq!(actual.repeatable(), option.repeatable);
        assert_eq!(actual.value_kind(), option.value_kind);
        assert_eq!(actual.required(), option.required);
    }
    for ((name, kind), value) in expected.required.iter().zip(required_values) {
        assert_eq!(admitted.required_kind(name), *kind);
        match kind {
            RequiredArgumentKind::Text => assert_eq!(
                admitted.required_text(name),
                value,
                "text positional {name} must be available only as admitted text"
            ),
            RequiredArgumentKind::Json => assert_eq!(
                admitted.required_json(name),
                &serde_json::from_str::<Value>(value).expect("oracle JSON must be valid"),
                "JSON positional {name} must be stored as an owned parsed value"
            ),
        }
    }
    for option in present_options {
        match (option.arity, option.value_kind) {
            (OptionArity::Boolean, _) => assert!(admitted.option_flag(option.name)),
            (OptionArity::Value, RequiredArgumentKind::Text) => assert_eq!(
                admitted.option_text(option.name),
                Some(sample_option_value(*option).as_str())
            ),
            (OptionArity::Value, RequiredArgumentKind::Json) => assert_eq!(
                admitted.option_json(option.name),
                Some(&json!({"oracle": option.name}))
            ),
        }
    }
}

fn append_option(args: &mut Vec<String>, option: OptionAuthority) {
    args.push(format!("--{}", option.name));
    if option.arity == OptionArity::Value {
        args.push(sample_option_value(option));
    }
}

fn sample_option_value(option: OptionAuthority) -> String {
    match option.value_kind {
        RequiredArgumentKind::Text => format!("accepted-{}", option.name),
        RequiredArgumentKind::Json => json!({"oracle": option.name}).to_string(),
    }
}

fn constraint_violation_args(
    route: &RouteAuthority,
    minimum: &[String],
    present: &[OptionAuthority],
    constraint: ConstraintAuthority,
) -> Vec<String> {
    let mut selected = present
        .iter()
        .copied()
        .filter(|option| {
            !constraint.members.contains(&option.name)
                && Some(option.name) != constraint.condition_option
                && Some(option.name) != constraint.required_option
        })
        .collect::<Vec<_>>();
    let mut args = minimum.to_vec();
    match constraint.kind {
        OptionConstraintKind::OneOf | OptionConstraintKind::MutuallyExclusive => {
            for name in constraint.members.iter().take(2) {
                selected.push(
                    *route
                        .options
                        .iter()
                        .find(|option| option.name == *name)
                        .expect("constraint member must name a documented option"),
                );
            }
        }
        OptionConstraintKind::AtLeastOne => {}
        OptionConstraintKind::ConditionalRequired => {
            for option in &selected {
                append_option(&mut args, *option);
            }
            args.extend([
                format!(
                    "--{}",
                    constraint
                        .condition_option
                        .expect("conditional option must be named")
                ),
                constraint
                    .condition_value
                    .expect("conditional value must be named")
                    .to_string(),
            ]);
            return args;
        }
    }
    for option in selected {
        append_option(&mut args, option);
    }
    args
}

fn route_authorities() -> Vec<RouteAuthority> {
    use CommandCardinality::{Exact, Options};
    use RequiredArgumentKind::{Json, Text};

    let mut routes = Vec::with_capacity(AUTHORITATIVE_ROUTE_COUNT);
    for (module, handler, path) in [
        ("mod.rs", "handle_commands", "commands"),
        ("model_registry.rs", "handle_read", "model-registry read"),
        (
            "model_registry.rs",
            "handle_refresh",
            "model-registry refresh",
        ),
        ("native_rpc.rs", "handle_rpc_call", "rpc call"),
        (
            "subagents.rs",
            "handle_subagents_catalog",
            "subagents catalog",
        ),
        (
            "subagents.rs",
            "handle_subagents_execute",
            "subagents execute",
        ),
        ("subagents.rs", "handle_mcp_start", "mcp start"),
        ("subagents.rs", "handle_mcp_stop", "mcp stop"),
        ("subagents.rs", "handle_mcp_reload", "mcp reload"),
        ("subagents.rs", "handle_mcp_status", "mcp status"),
    ] {
        add_authority_routes(&mut routes, module, handler, &[path], Exact);
    }

    add_authority_routes(
        &mut routes,
        "adapter.rs",
        "handle_catalog",
        &["adapter catalog"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "adapter.rs",
        "handle_antigravity_status",
        &["adapter antigravity status"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "adapter.rs",
        "handle_antigravity_install",
        &["adapter antigravity install"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "adapter.rs",
        "handle_antigravity_uninstall",
        &["adapter antigravity uninstall"],
        Exact,
    );
    routes.push(RouteAuthority {
        module: "adapter.rs",
        handler: "handle_antigravity_authorize",
        path: "adapter antigravity authorize",
        required: &[],
        cardinality: Options,
        options: vec![value_option("binary-path", Text, false)],
        constraints: &[],
    });
    add_authority_routes(
        &mut routes,
        "adapter.rs",
        "handle_subagent_mcp_status",
        &["adapter subagent-mcp status"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "adapter.rs",
        "handle_subagent_mcp_plan",
        &["adapter subagent-mcp plan"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "adapter.rs",
        "handle_subagent_mcp_install",
        &["adapter subagent-mcp install"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "agent_conversation.rs",
        "handle_agent_conversation",
        &[
            "agent conversation open",
            "agent conversation send",
            "agent conversation steer",
            "agent conversation cancel",
            "agent conversation capabilities",
            "agent conversation stream",
            "agent conversation cleanup",
        ],
        Options,
    );
    routes.push(RouteAuthority {
        module: "client_conversation.rs",
        handler: "handle_conversation_execute",
        path: "conversation execute",
        required: &[],
        cardinality: Options,
        options: vec![
            value_option("stdin-json", Json, true),
            boolean_option("require-running-host"),
        ],
        constraints: &[],
    });
    add_authority_routes(
        &mut routes,
        "conversation_surface.rs",
        "handle_conversation_list",
        &["conversation list"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "conversation_surface.rs",
        "handle_conversation_get",
        &["conversation get"],
        Options,
    );
    routes.push(RouteAuthority {
        module: "strategy.rs",
        handler: "handle_strategy_execute",
        path: "strategy execute",
        required: &[],
        cardinality: Options,
        options: vec![value_option("stdin-json", Json, true)],
        constraints: &[],
    });
    add_authority_routes(
        &mut routes,
        "agent_conversation.rs",
        "handle_agents_pair",
        &[
            "agents pair request",
            "agents pair approve",
            "agents pair revoke",
            "agents pair list",
        ],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "agent_hub.rs",
        "handle_catalog",
        &["agent-hub catalog"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "agent_hub.rs",
        "handle_plan",
        &["agent-hub plan"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "agent_hub.rs",
        "handle_apply",
        &["agent-hub apply"],
        Options,
    );
    routes.push(RouteAuthority {
        module: "extension_host.rs",
        handler: "handle_serve",
        path: "extension-host serve",
        required: &[("data-root", Text), ("package-id", Text), ("version", Text)],
        cardinality: Exact,
        options: options_for_route("extension-host serve"),
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_catalog",
        path: "package catalog",
        required: &[("data-root", Text)],
        cardinality: Options,
        options: options_for_route("package catalog"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_install_plan",
        path: "package install-plan",
        required: &[("data-root", Text)],
        cardinality: Options,
        options: options_for_route("package install-plan"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_install_confirm",
        path: "package install-confirm",
        required: &[("data-root", Text)],
        cardinality: Options,
        options: options_for_route("package install-confirm"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_install_apply",
        path: "package install-apply",
        required: &[("data-root", Text)],
        cardinality: Options,
        options: options_for_route("package install-apply"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_import",
        path: "package import",
        required: &[("data-root", Text)],
        cardinality: Exact,
        options: options_for_route("package import"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_enable",
        path: "package enable",
        required: &[("data-root", Text), ("package-id", Text), ("version", Text)],
        cardinality: Exact,
        options: options_for_route("package enable"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_disable",
        path: "package disable",
        required: &[("data-root", Text), ("package-id", Text), ("version", Text)],
        cardinality: Exact,
        options: options_for_route("package disable"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_uninstall_preview",
        path: "package uninstall-preview",
        required: &[("data-root", Text), ("package-id", Text), ("version", Text)],
        cardinality: Options,
        options: options_for_route("package uninstall-preview"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_uninstall_drain",
        path: "package uninstall-drain",
        required: &[("data-root", Text), ("package-id", Text), ("version", Text)],
        cardinality: Options,
        options: options_for_route("package uninstall-drain"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_uninstall_collect",
        path: "package uninstall-collect",
        required: &[("data-root", Text), ("package-id", Text), ("version", Text)],
        cardinality: Exact,
        options: options_for_route("package uninstall-collect"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_recover",
        path: "package recover",
        required: &[("data-root", Text)],
        cardinality: Options,
        options: options_for_route("package recover"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_update_preview",
        path: "package update-preview",
        required: &[("data-root", Text), ("package-id", Text)],
        cardinality: Options,
        options: options_for_route("package update-preview"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_update_apply",
        path: "package update-apply",
        required: &[("data-root", Text), ("package-id", Text)],
        cardinality: Exact,
        options: options_for_route("package update-apply"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_activate",
        path: "package activate",
        required: &[("data-root", Text), ("package-id", Text), ("version", Text)],
        cardinality: Options,
        options: options_for_route("package activate"),
        constraints: &[],
    });

    routes.push(RouteAuthority {
        module: "package.rs",
        handler: "handle_resource_select",
        path: "package resource-select",
        required: &[("data-root", Text), ("package-id", Text), ("version", Text)],
        cardinality: Options,
        options: options_for_route("package resource-select"),
        constraints: &[],
    });

    add_authority_routes(
        &mut routes,
        "agent_usage.rs",
        "handle_agent_usage_scan",
        &["agent-usage scan"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "agent_usage.rs",
        "handle_agent_usage_report",
        &["agent-usage report"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "provider_quota.rs",
        "handle_provider_quota_snapshot",
        &["provider-quota snapshot"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "resource_usage.rs",
        "handle_resource_usage_scan",
        &["resource-usage scan"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "client_update.rs",
        "handle_update",
        &[
            "update status",
            "update check",
            "update download",
            "update verify",
            "update apply",
        ],
        Options,
    );
    for (path, handler) in [
        ("collaboration status", "handle_status"),
        ("collaboration enable", "handle_enable"),
        ("collaboration install plan", "handle_install_plan"),
        ("collaboration install apply", "handle_install_apply"),
        ("collaboration install cancel", "handle_install_cancel"),
        ("collaboration workflow catalog", "handle_workflow_catalog"),
        (
            "collaboration workflow local-deployment plan",
            "handle_local_deployment_plan",
        ),
        (
            "collaboration workflow local-deployment apply",
            "handle_local_deployment_apply",
        ),
        (
            "collaboration workflow mcp-install plan",
            "handle_mcp_install_plan",
        ),
        (
            "collaboration workflow mcp-install apply",
            "handle_mcp_install_apply",
        ),
        ("collaboration workflow cancel", "handle_workflow_cancel"),
        (
            "collaboration local-server status",
            "handle_local_server_status",
        ),
        (
            "collaboration local-server start",
            "handle_local_server_start",
        ),
        (
            "collaboration local-server stop",
            "handle_local_server_stop",
        ),
        (
            "collaboration local-server uninstall",
            "handle_local_server_uninstall",
        ),
        ("collaboration disable", "handle_disable"),
        ("collaboration cleanup", "handle_cleanup"),
    ] {
        add_authority_routes(&mut routes, "collaboration.rs", handler, &[path], Options);
    }
    add_authority_routes(
        &mut routes,
        "mcp.rs",
        "handle_preview",
        &["mcp http preview"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "mcp.rs",
        "handle_execute",
        &["mcp http execute"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "mobile.rs",
        "handle_mobile_relay",
        &[
            "mobile relay config get",
            "mobile relay config set",
            "mobile relay pairing create",
            "mobile relay pairing claim",
            "mobile relay pairing status",
            "mobile relay pairing revoke",
            "mobile relay pc check-in",
            "mobile relay commands poll",
            "mobile relay commands sync",
            "mobile relay commands create",
            "mobile relay commands create-secure",
            "mobile relay commands result",
            "mobile relay commands result-secure",
            "mobile relay commands result-replay-proof",
            "mobile relay e2ee secret-store-cleanup",
            "mobile relay e2ee secret-store-cleanup-inventory",
            "mobile relay e2ee secret-store-self-test",
        ],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "llm_gateway.rs",
        "handle_status",
        &["llm-gateway credentials status"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "gateway.rs",
        "handle_help",
        &["gateway help"],
        Exact,
    );
    routes.push(RouteAuthority {
        module: "gateway.rs",
        handler: "handle_client_token",
        path: "gateway client-token",
        required: &[],
        cardinality: Options,
        options: vec![value_option("agent", Text, true)],
        constraints: &[],
    });
    for (path, handler) in [
        ("gateway service status", "handle_service_status"),
        ("gateway service start", "handle_service_start"),
        ("gateway service stop", "handle_service_stop"),
        ("gateway service initialize", "handle_service_initialize"),
    ] {
        add_authority_routes(&mut routes, "gateway.rs", handler, &[path], Options);
    }
    routes.push(RouteAuthority {
        module: "gateway.rs",
        handler: "handle_inventory_reload",
        path: "gateway inventory reload",
        required: &[],
        cardinality: Options,
        options: vec![value_option("stdin-json", Json, true)],
        constraints: &[],
    });
    for (path, handler) in [
        ("gateway channel status", "handle_channel_status"),
        (
            "gateway channel telegram credentials status",
            "handle_telegram_credentials_status",
        ),
        (
            "gateway channel telegram credentials clear",
            "handle_telegram_credentials_clear",
        ),
        (
            "gateway channel telegram pairing list",
            "handle_telegram_pairing_list",
        ),
    ] {
        add_authority_routes(&mut routes, "gateway.rs", handler, &[path], Exact);
    }
    routes.push(RouteAuthority {
        module: "gateway.rs",
        handler: "handle_telegram_credentials_set",
        path: "gateway channel telegram credentials set",
        required: &[],
        cardinality: Options,
        options: vec![value_option("stdin-json", Json, true)],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "gateway.rs",
        handler: "handle_telegram_pairing_approve",
        path: "gateway channel telegram pairing approve",
        required: &[("code", Text)],
        cardinality: Exact,
        options: vec![],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "gateway.rs",
        handler: "handle_telegram_pairing_revoke",
        path: "gateway channel telegram pairing revoke",
        required: &[("chat-id", Text)],
        cardinality: Exact,
        options: vec![],
        constraints: &[],
    });
    add_authority_routes(
        &mut routes,
        "llm_gateway.rs",
        "handle_list",
        &["llm-gateway credentials list"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "llm_gateway.rs",
        "handle_migrate",
        &["llm-gateway credentials migrate"],
        Exact,
    );
    routes.push(RouteAuthority {
        module: "llm_gateway.rs",
        handler: "handle_authorize",
        path: "llm-gateway credentials authorize",
        required: &[],
        cardinality: Exact,
        options: vec![],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "llm_gateway.rs",
        handler: "handle_clear",
        path: "llm-gateway credentials clear",
        required: &[],
        cardinality: Exact,
        options: vec![],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "llm_gateway.rs",
        handler: "handle_create",
        path: "llm-gateway credentials create",
        required: &[],
        cardinality: Options,
        options: vec![value_option("stdin-json", Json, true)],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "llm_gateway.rs",
        handler: "handle_delete",
        path: "llm-gateway credentials delete",
        required: &[("credential-id", Text)],
        cardinality: Exact,
        options: vec![],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "llm_gateway.rs",
        handler: "handle_lease",
        path: "llm-gateway credentials lease",
        required: &[("days", Text)],
        cardinality: Exact,
        options: vec![],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "llm_gateway.rs",
        handler: "handle_update",
        path: "llm-gateway credentials update",
        required: &[("credential-id", Text)],
        cardinality: Options,
        options: vec![value_option("stdin-json", Json, true)],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "llm_gateway.rs",
        handler: "handle_agent_plan",
        path: "llm-gateway agent-config plan",
        required: &[("agent", Text), ("config-root", Text)],
        cardinality: Options,
        options: vec![value_option("port", Text, false)],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "llm_gateway.rs",
        handler: "handle_agent_apply",
        path: "llm-gateway agent-config apply",
        required: &[("agent", Text), ("config-root", Text)],
        cardinality: Options,
        options: vec![
            value_option("port", Text, false),
            value_option("confirmation", Text, true),
            OptionAuthority {
                name: "confirmed",
                arity: OptionArity::Boolean,
                repeatable: false,
                value_kind: Text,
                required: true,
            },
            value_option("stdin-json", Json, false),
        ],
        constraints: &[],
    });
    for (path, handler) in [
        ("llm-gateway service status", "handle_service_status"),
        (
            "llm-gateway service initialize",
            "handle_service_initialize",
        ),
        ("llm-gateway service start", "handle_service_start"),
        ("llm-gateway service stop", "handle_service_stop"),
        (
            "llm-gateway service autostart-enable",
            "handle_service_autostart_enable",
        ),
    ] {
        routes.push(RouteAuthority {
            module: "llm_gateway.rs",
            handler,
            path,
            required: &[],
            cardinality: Options,
            options: vec![value_option("port", Text, false)],
            constraints: &[],
        });
    }
    add_authority_routes(
        &mut routes,
        "llm_gateway.rs",
        "handle_service_usage",
        &["llm-gateway service usage"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "llm_gateway.rs",
        "handle_service_autostart_status",
        &["llm-gateway service autostart-status"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "llm_gateway.rs",
        "handle_service_autostart_disable",
        &["llm-gateway service autostart-disable"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "autostart.rs",
        "handle_status",
        &["autostart status"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "autostart.rs",
        "handle_prepare_mcp",
        &["autostart prepare-mcp"],
        Exact,
    );
    routes.push(RouteAuthority {
        module: "autostart.rs",
        handler: "handle_set",
        path: "autostart set",
        required: &[],
        cardinality: Options,
        options: vec![
            value_option("component", Text, true),
            value_option("enabled", Text, true),
            value_option("silent", Text, false),
            value_option("port", Text, false),
        ],
        constraints: &[],
    });
    add_authority_routes(
        &mut routes,
        "opencode_serve.rs",
        "handle_opencode_serve",
        &[
            "opencode-serve ensure",
            "opencode-serve start",
            "opencode-serve stop",
            "opencode-serve restart",
            "opencode-serve status",
        ],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "secure_mesh.rs",
        "handle_secure_mesh",
        &[
            "secure-mesh status",
            "secure-mesh envelope validate",
            "secure-mesh command policy",
            "secure-mesh command evaluate",
            "secure-mesh command execute",
            "secure-mesh device-trust evaluate",
            "secure-mesh file route",
            "secure-mesh file receive-destination",
            "secure-mesh file receive-confirmation",
            "secure-mesh approval request",
            "secure-mesh approval fanout",
            "secure-mesh approval respond",
            "secure-mesh approval inbox",
            "secure-mesh approval adapter-capability",
        ],
        Options,
    );
    for (path, handler) in [
        ("skill list", "handle_skill_list"),
        ("skill get", "handle_skill_get"),
        ("skill delete plan", "handle_skill_delete_plan"),
        ("skill delete apply", "handle_skill_delete_apply"),
        ("skill visibility set", "handle_skill_visibility"),
        ("skill usage report", "handle_skill_usage_report"),
        ("skill usage scan", "handle_skill_usage_scan"),
    ] {
        add_authority_routes(&mut routes, "skill.rs", handler, &[path], Options);
    }
    add_authority_routes(
        &mut routes,
        "snapshots.rs",
        "handle_snapshots_list",
        &["snapshots list"],
        Options,
    );
    routes.push(RouteAuthority {
        module: "snapshots.rs",
        handler: "handle_snapshots_restore",
        path: "snapshots restore",
        required: &[("snapshot-id", Text)],
        cardinality: Exact,
        options: vec![],
        constraints: &[],
    });
    add_authority_routes(
        &mut routes,
        "snapshots.rs",
        "handle_snapshots_collect",
        &["snapshots collect"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "snapshots.rs",
        "handle_snapshots_root",
        &["snapshots root get", "snapshots root set"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "snapshots.rs",
        "handle_snapshots_profiles",
        &[
            "snapshots profiles list",
            "snapshots profiles get",
            "snapshots profiles import",
        ],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "snapshots.rs",
        "handle_snapshots_archive",
        &[
            "snapshots archive collect",
            "snapshots archive run",
            "snapshots archive verify",
            "snapshots archive report",
            "snapshots archive jobs preview",
            "snapshots archive jobs create",
            "snapshots archive jobs status",
            "snapshots archive jobs list",
            "snapshots archive jobs events",
            "snapshots archive jobs cancel",
            "snapshots archive jobs drain",
        ],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "snapshots.rs",
        "handle_snapshots_collections",
        &["snapshots collections list"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "snapshots.rs",
        "handle_conversations",
        &[
            "conversations list",
            "conversations stream",
            "conversations append",
            "conversations delete",
        ],
        Options,
    );
    routes.extend([
        RouteAuthority {
            module: "state.rs",
            handler: "handle_state_get",
            path: "state get",
            required: &[("collection", Text)],
            cardinality: Exact,
            options: vec![],
            constraints: &[],
        },
        RouteAuthority {
            module: "state.rs",
            handler: "handle_state_set",
            path: "state set",
            required: &[("collection", Text), ("payload", Json)],
            cardinality: Exact,
            options: vec![],
            constraints: &[],
        },
        RouteAuthority {
            module: "state.rs",
            handler: "handle_state_admit",
            path: "state admit",
            required: &[("data-root", Text)],
            cardinality: Exact,
            options: vec![],
            constraints: &[],
        },
    ]);
    add_authority_routes(
        &mut routes,
        "state.rs",
        "handle_activity_list",
        &["activity list"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "targets.rs",
        "handle_targets_catalog",
        &["targets catalog"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "targets.rs",
        "handle_targets_scan",
        &["targets scan"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "targets.rs",
        "handle_targets_add",
        &["targets add"],
        Options,
    );
    routes.push(RouteAuthority {
        module: "targets.rs",
        handler: "handle_targets_inspect",
        path: "targets inspect",
        required: &[("target", Text)],
        cardinality: Options,
        options: vec![],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "targets.rs",
        handler: "handle_targets_model_selection",
        path: "targets model-selection",
        required: &[("target", Text)],
        cardinality: Options,
        options: vec![],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "full_backup.rs",
        handler: "handle_backup_export",
        path: "backup export",
        required: &[("archive", Text)],
        cardinality: Options,
        options: vec![],
        constraints: &[],
    });
    routes.push(RouteAuthority {
        module: "full_backup.rs",
        handler: "handle_backup_import",
        path: "backup import",
        required: &[("archive", Text)],
        cardinality: Options,
        options: vec![],
        constraints: &[],
    });
    add_authority_routes(
        &mut routes,
        "project.rs",
        "handle_project_register",
        &["project register"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "project.rs",
        "handle_project_read",
        &["project read"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "project.rs",
        "handle_project_list",
        &["project list"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "project.rs",
        "handle_project_import_preview",
        &["project import-preview"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "project.rs",
        "handle_project_import_apply",
        &["project import-apply"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "project.rs",
        "handle_project_dependency_declare",
        &["project dependency declare"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "project.rs",
        "handle_project_dependency_list",
        &["project dependency list"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "project.rs",
        "handle_project_dependency_unresolved",
        &["project dependency unresolved"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "project.rs",
        "handle_project_dependency_blocked",
        &["project dependency blocked"],
        Exact,
    );
    // The cross-device entry's own routes are part of the published registry, so
    // the authority is incomplete without them: the projection comparison below
    // is between this list and the live table, not a subset of it.
    add_authority_routes(
        &mut routes,
        "mobile_peer.rs",
        "handle_mobile_peer",
        &["mobile relay peer status"],
        Exact,
    );
    add_authority_routes(
        &mut routes,
        "mobile_peer.rs",
        "handle_mobile_peer",
        &["mobile relay peer protocol-line"],
        Options,
    );
    add_authority_routes(
        &mut routes,
        "mobile_peer.rs",
        "handle_mobile_peer",
        &[
            "mobile relay peer record",
            "mobile relay peer bind",
            "mobile relay peer revoke",
        ],
        Options,
    );
    for route in &mut routes {
        route.required = match route.path {
            "skill get" | "skill visibility set" => &[("skill-id", Text)],
            "project read" | "project dependency list" | "project dependency unresolved" => {
                &[("project-id", Text)]
            }
            "project dependency blocked" => &[("project-id", Text), ("work-item-id", Text)],
            "rpc call" => &[("method", Text)],
            _ => route.required,
        };
        route.options = options_for_route(route.path);
        route.constraints = constraints_for_route(route.path);
        route.cardinality = if route.options.is_empty() {
            Exact
        } else {
            Options
        };
    }
    routes
}

fn add_authority_routes(
    routes: &mut Vec<RouteAuthority>,
    module: &'static str,
    handler: &'static str,
    paths: &[&'static str],
    cardinality: CommandCardinality,
) {
    routes.extend(paths.iter().copied().map(|path| RouteAuthority {
        module,
        handler,
        path,
        required: &[],
        cardinality,
        options: vec![],
        constraints: &[],
    }));
}

const fn value_option(
    name: &'static str,
    value_kind: RequiredArgumentKind,
    required: bool,
) -> OptionAuthority {
    OptionAuthority {
        name,
        arity: OptionArity::Value,
        repeatable: false,
        value_kind,
        required,
    }
}

const fn boolean_option(name: &'static str) -> OptionAuthority {
    OptionAuthority {
        name,
        arity: OptionArity::Boolean,
        repeatable: false,
        value_kind: RequiredArgumentKind::Text,
        required: false,
    }
}

fn options_for_route(path: &str) -> Vec<OptionAuthority> {
    use RequiredArgumentKind::{Json, Text};
    let options: &[OptionAuthority] = match path {
        "rpc call"
        | "subagents execute"
        | "project register"
        | "project import-preview"
        | "project dependency declare"
        | "mobile relay peer record"
        | "mobile relay peer bind"
        | "mobile relay peer revoke" => &[value_option("stdin-json", Json, true)],
        "mobile relay peer protocol-line" => &[value_option("authority-file", Text, true)],
        "project import-apply" => &[
            value_option("stdin-json", Json, true),
            value_option("expected-revision", Text, true),
        ],
        "mcp start" | "mcp reload" => &[value_option("binary", Text, false)],
        "gateway client-token" => &[value_option("agent", Text, true)],
        "gateway service status"
        | "gateway service start"
        | "gateway service stop"
        | "gateway service initialize" => &[value_option("port", Text, false)],
        "gateway inventory reload" | "gateway channel telegram credentials set" => {
            &[value_option("stdin-json", Json, true)]
        }
        "llm-gateway credentials authorize" | "llm-gateway credentials clear" => {
            &[value_option("credential-id", Text, false)]
        }
        "llm-gateway credentials create" | "llm-gateway credentials update" => {
            &[value_option("stdin-json", Json, true)]
        }
        "llm-gateway agent-config plan" => &[value_option("port", Text, false)],
        "llm-gateway agent-config apply" => &[
            value_option("port", Text, false),
            value_option("confirmation", Text, true),
            OptionAuthority {
                name: "confirmed",
                arity: OptionArity::Boolean,
                repeatable: false,
                value_kind: Text,
                required: true,
            },
            value_option("stdin-json", Json, false),
        ],
        "llm-gateway service status"
        | "llm-gateway service initialize"
        | "llm-gateway service start"
        | "llm-gateway service stop"
        | "llm-gateway service autostart-enable" => &[value_option("port", Text, false)],
        "autostart set" => &[
            value_option("component", Text, true),
            value_option("enabled", Text, true),
            value_option("silent", Text, false),
            value_option("port", Text, false),
        ],
        "opencode-serve ensure"
        | "opencode-serve start"
        | "opencode-serve stop"
        | "opencode-serve restart"
        | "opencode-serve status" => &[
            value_option("port", Text, false),
            value_option("executable", Text, false),
            value_option("attach-url", Text, false),
        ],
        "activity list" => &[
            value_option("type", Text, false),
            value_option("target", Text, false),
            value_option("limit", Text, false),
        ],
        "snapshots list" => &[value_option("target", Text, false)],
        "snapshots root get" | "snapshots root set" => &[value_option("path", Text, false)],
        "snapshots collections list" => &[value_option("snapshot-root", Text, false)],
        "snapshots profiles list" | "snapshots profiles get" | "snapshots profiles import" => &[
            value_option("profile", Text, false),
            value_option("profile-json", Json, false),
            value_option("profile-file", Text, false),
        ],
        "snapshots archive jobs preview" => &[
            value_option("selection-mode", Text, true),
            value_option("query", Text, false),
            value_option("path", Text, true),
            value_option("agent", Text, false),
        ],
        "snapshots archive jobs create" => &[
            value_option("selection-mode", Text, true),
            value_option("query", Text, false),
            value_option("path", Text, true),
            value_option("plan-binding", Text, true),
            value_option("agent", Text, false),
        ],
        "snapshots archive jobs status"
        | "snapshots archive jobs events"
        | "snapshots archive jobs cancel" => &[value_option("job-id", Text, true)],
        "snapshots archive jobs list" | "snapshots archive jobs drain" => &[
            value_option("job-id", Text, false),
            value_option("once", Text, false),
        ],
        "snapshots archive collect" => &[
            value_option("keywords", Text, true),
            value_option("path", Text, true),
            value_option("trigger", Text, false),
        ],
        "snapshots archive run" | "snapshots archive report" => &[
            value_option("profile", Text, true),
            value_option("trigger", Text, false),
        ],
        "snapshots archive verify" => &[
            value_option("profile", Text, false),
            value_option("trigger", Text, false),
            value_option("collection-path", Text, false),
        ],
        "snapshots collect" => &[
            value_option("topic", Text, true),
            value_option("agent", Text, false),
        ],
        "backup export" => &[
            value_option("data-root", Text, false),
            boolean_option("writers-stopped"),
        ],
        "backup import" => &[value_option("target-root", Text, true)],
        "conversations list" | "conversations stream" => &[
            value_option("agent", Text, true),
            value_option("limit", Text, false),
            value_option("offset", Text, false),
            value_option("session-id", Text, false),
            value_option("text", Text, false),
            value_option("stdin-json", Json, false),
        ],
        "conversations append" | "conversations delete" => &[
            value_option("agent", Text, true),
            value_option("limit", Text, false),
            value_option("offset", Text, false),
            value_option("session-id", Text, false),
            value_option("text", Text, false),
            value_option("stdin-json", Json, false),
        ],
        "agent-hub catalog" => &[
            value_option("agent-id", Text, false),
            value_option("stdin-json", Json, false),
        ],
        "agent-hub plan" => &[
            value_option("agent-id", Text, true),
            value_option("operation", Text, false),
            value_option("stdin-json", Json, false),
        ],
        "agent-hub apply" => &[
            value_option("agent-id", Text, true),
            value_option("confirmation", Text, true),
            value_option("operation", Text, false),
            value_option("stdin-json", Json, false),
            boolean_option("cancel"),
        ],
        "extension-host serve" => &[
            value_option("await-ms", Text, false),
            value_option("cancel-after-ms", Text, false),
            value_option("capability", Text, false),
            value_option("mode", Text, false),
            value_option("request", Json, false),
        ],
        "package catalog"
        | "package enable"
        | "package disable"
        | "package uninstall-preview"
        | "package recover"
        | "package activate" => &[],
        "package resource-select" => &[
            value_option("kind", Text, true),
            value_option("resource", Text, true),
        ],
        "package install-plan" | "package import" => &[value_option("archive", Text, true)],
        "package install-confirm" => &[
            value_option("archive", Text, true),
            value_option("plan", Text, true),
        ],
        "package install-apply" => &[
            value_option("archive", Text, true),
            value_option("confirmation", Text, true),
        ],
        "package uninstall-drain" => &[
            value_option("remaining", Text, false),
            value_option("dependents", Json, false),
            value_option("instances", Json, false),
        ],
        "package uninstall-collect" => &[value_option("registration-inputs", Json, false)],
        "package update-preview" => &[value_option("archive", Text, false)],
        "package update-apply" => &[
            value_option("archive", Text, true),
            value_option("confirmation", Text, true),
        ],
        "agent-usage scan" => &[
            value_option("agent", Text, false),
            value_option("history-days", Text, false),
            value_option("timezone-offset-minutes", Text, false),
            value_option("timezone-transitions-json", Json, false),
            boolean_option("force-refresh"),
            value_option("state-root", Text, false),
        ],
        "agent-usage report" => &[
            value_option("agent", Text, false),
            value_option("limit", Text, false),
            value_option("state-root", Text, false),
        ],
        "provider-quota snapshot" => &[
            value_option("agent", Text, false),
            boolean_option("force-refresh"),
            value_option("state-root", Text, false),
        ],
        "resource-usage scan" => &[value_option("state-root", Text, false)],
        "adapter antigravity authorize" => &[value_option("binary-path", Text, false)],
        "adapter subagent-mcp status" | "adapter subagent-mcp plan" => &[
            value_option("agent-id", Text, true),
            value_option("binary-path", Text, false),
            value_option("mcp-binary-path", Text, false),
            value_option("config-path", Text, false),
        ],
        "adapter subagent-mcp install" => &[
            value_option("agent-id", Text, true),
            value_option("binary-path", Text, false),
            value_option("mcp-binary-path", Text, false),
            value_option("config-path", Text, false),
            value_option("confirmation", Text, true),
            OptionAuthority {
                name: "confirmed",
                arity: OptionArity::Boolean,
                repeatable: false,
                value_kind: Text,
                required: true,
            },
        ],
        "mcp http preview" | "mcp http execute" => &[value_option("stdin-json", Json, true)],
        "update status" | "update check" => &[
            value_option("target-release-track", Text, false),
            value_option("manifest-path", Text, false),
            value_option("public-keys-path", Text, false),
            value_option("revocation-path", Text, false),
            value_option("source-path", Text, false),
            value_option("source", Text, false),
            value_option("repo", Text, false),
            value_option("staging-root", Text, false),
            value_option("state-root", Text, false),
            value_option("execute", Text, false),
            value_option("install-root", Text, false),
            value_option("gui-pid", Text, false),
            value_option("wait-for-script", Text, false),
        ],
        "update download" | "update verify" => &[
            value_option("manifest-path", Text, false),
            value_option("public-keys-path", Text, false),
            value_option("revocation-path", Text, false),
            value_option("source-path", Text, false),
            value_option("source", Text, false),
            value_option("repo", Text, false),
            value_option("staging-root", Text, false),
            value_option("state-root", Text, false),
            value_option("execute", Text, false),
            value_option("install-root", Text, false),
            value_option("gui-pid", Text, false),
            value_option("wait-for-script", Text, false),
        ],
        "update apply" => &[
            value_option("manifest-path", Text, false),
            value_option("public-keys-path", Text, false),
            value_option("revocation-path", Text, false),
            value_option("source-path", Text, false),
            value_option("source", Text, false),
            value_option("repo", Text, false),
            value_option("staging-root", Text, false),
            value_option("state-root", Text, false),
            value_option("data-root", Text, false),
            value_option("execute", Text, false),
            value_option("install-root", Text, false),
            value_option("gui-pid", Text, false),
            value_option("wait-for-script", Text, false),
        ],
        "collaboration install plan"
        | "collaboration install apply"
        | "collaboration install cancel" => &[
            value_option("github-url", Text, false),
            value_option("plan-id", Text, false),
            value_option("expected-digest-sha256", Text, false),
            value_option("confirmed", Text, false),
        ],
        "collaboration workflow local-deployment plan"
        | "collaboration workflow local-deployment apply" => &[
            value_option("request-origin", Text, true),
            value_option("selected-feature-ids", Text, true),
            value_option("destination", Text, true),
            value_option("destination-confirmed", Text, true),
            value_option("port", Text, false),
            value_option("plan-id", Text, false),
            value_option("expected-plan-digest-sha256", Text, false),
            value_option("expected-package-digest-sha256", Text, false),
            value_option("confirmed", Text, false),
        ],
        "collaboration local-server start" | "collaboration local-server stop" => &[
            value_option("request-origin", Text, true),
            value_option("deployment-id", Text, true),
            value_option("confirmed", Text, true),
        ],
        "collaboration local-server uninstall" => &[
            value_option("request-origin", Text, true),
            value_option("deployment-id", Text, true),
            value_option("expected-assembly-manifest-digest-sha256", Text, true),
            value_option("confirmed", Text, true),
        ],
        "collaboration workflow mcp-install plan" | "collaboration workflow mcp-install apply" => {
            &[
                value_option("request-origin", Text, true),
                value_option("selected-plugin-ids", Text, true),
                value_option("agent-destinations", Json, true),
                value_option("plan-id", Text, false),
                value_option("expected-plan-digest-sha256", Text, false),
                value_option("expected-package-digest-sha256", Text, false),
                value_option("confirmed", Text, false),
            ]
        }
        "collaboration workflow cancel" => &[
            value_option("request-origin", Text, true),
            value_option("plan-id", Text, true),
            value_option("expected-plan-digest-sha256", Text, true),
            value_option("expected-package-digest-sha256", Text, true),
            value_option("confirmed", Text, true),
        ],
        "agent conversation open"
        | "agent conversation send"
        | "agent conversation steer"
        | "agent conversation cancel"
        | "agent conversation cleanup"
        | "agent conversation capabilities"
        | "agent conversation stream" => &[value_option("stdin-json", Json, false)],
        "conversation execute" => &[
            value_option("stdin-json", Json, true),
            boolean_option("require-running-host"),
        ],
        "conversation list" => &[boolean_option("include-archived")],
        "conversation get" => &[value_option("conversation-id", Text, true)],
        "strategy execute" => &[value_option("stdin-json", Json, true)],
        "agents pair request"
        | "agents pair approve"
        | "agents pair revoke"
        | "agents pair list" => &[
            value_option("agent", Text, true),
            value_option("target", Text, false),
        ],
        "skill list" | "skill get" => &[
            value_option("agent", Text, true),
            value_option("skill-root", Text, false),
        ],
        "skill delete plan" => &[
            value_option("skill", Text, true),
            value_option("path", Text, true),
        ],
        "skill delete apply" => &[
            value_option("skill", Text, true),
            value_option("path", Text, true),
            value_option("confirmation", Text, true),
        ],
        "skill visibility set" => &[
            value_option("agent", Text, true),
            value_option("hidden", Text, true),
        ],
        "skill usage report" => &[
            value_option("agent", Text, false),
            value_option("skill", Text, false),
            value_option("days", Text, false),
            value_option("from", Text, false),
            value_option("to", Text, false),
        ],
        "skill usage scan" => &[
            value_option("agent", Text, false),
            value_option("history-root", Text, false),
            value_option("home-dir", Text, false),
            boolean_option("force-refresh"),
        ],
        "targets scan" => &[
            value_option("state-root", Text, false),
            value_option("include-accessible-environments", Text, false),
            value_option("include-history-model-catalog", Text, false),
            value_option("enable-agent-cli-model-lookup", Text, false),
            value_option("stdin-json", Json, false),
        ],
        "targets add" => &[
            value_option("target", Text, true),
            value_option("config-path", Text, false),
            value_option("binary-path", Text, false),
            value_option("history-root", Text, false),
            value_option("state-root", Text, false),
            value_option("stdin-json", Json, false),
        ],
        "targets model-selection" => &[
            value_option("state-root", Text, false),
            value_option("include-accessible-environments", Text, false),
            value_option("enable-agent-cli-model-lookup", Text, false),
        ],
        "targets inspect" => &[
            value_option("state-root", Text, false),
            value_option("include-accessible-environments", Text, false),
            value_option("enable-agent-cli-model-lookup", Text, false),
        ],
        "mobile relay config get" => &[
            value_option("authorize", Text, false),
            value_option("hydrate-secrets", Text, false),
        ],
        "mobile relay config set" => &[
            value_option("stdin-json", Json, false),
            value_option("station-base-url", Text, false),
            value_option("relay-enabled", Text, false),
            value_option("pc-client-id", Text, false),
            value_option("pc-client-name", Text, false),
            value_option("pairing-id", Text, false),
            value_option("paired", Text, false),
            value_option("reset-pairing", Text, false),
        ],
        "mobile relay pairing create"
        | "mobile relay pairing status"
        | "mobile relay pairing revoke" => &[
            value_option("stdin-json", Json, false),
            value_option("pairing-id", Text, false),
        ],
        "mobile relay pairing claim" => &[
            value_option("stdin-json", Json, false),
            value_option("pairing-id", Text, false),
        ],
        "mobile relay commands poll"
        | "mobile relay commands create"
        | "mobile relay commands result"
        | "mobile relay commands result-replay-proof" => &[
            value_option("command-id", Text, false),
            value_option("type", Text, false),
            value_option("stdin-json", Json, false),
        ],
        "mobile relay commands create-secure" => &[
            value_option("client-intent-id", Text, false),
            value_option("command-kind", Text, false),
            value_option("target-agent-id", Text, false),
            value_option("workspace-id", Text, false),
            value_option("body", Json, false),
            value_option("station-base-url", Text, false),
            value_option("allow-interaction", Text, false),
            value_option("stdin-json", Json, false),
        ],
        "mobile relay commands result-secure" => &[
            value_option("command-id", Text, false),
            value_option("idempotency-key", Text, false),
            value_option("acknowledge-receipt-id", Text, false),
            value_option("type", Text, false),
            value_option("stdin-json", Json, false),
        ],
        "mobile relay commands sync" => &[
            value_option("allow-interaction", Text, false),
            value_option("command-id", Text, false),
            value_option("type", Text, false),
            value_option("stdin-json", Json, false),
        ],
        "mobile relay e2ee secret-store-cleanup" => {
            &[value_option("cleanup-confirmation", Json, true)]
        }
        "secure-mesh status"
        | "secure-mesh envelope validate"
        | "secure-mesh command policy"
        | "secure-mesh command evaluate"
        | "secure-mesh command execute" => &[
            value_option("payload", Json, false),
            value_option("context", Json, false),
            value_option("ledger-path", Text, false),
        ],
        "secure-mesh device-trust evaluate" => &[
            value_option("identity", Json, true),
            value_option("previous-identity", Json, false),
            value_option("trust-state", Text, false),
        ],
        "secure-mesh file route" => &[value_option("manifest", Json, true)],
        "secure-mesh file receive-destination" => &[
            value_option("manifest", Json, true),
            value_option("approved-root", Text, true),
            value_option("conflict-policy", Text, false),
        ],
        "secure-mesh file receive-confirmation" => &[
            value_option("manifest", Json, true),
            value_option("approved-root", Text, true),
            value_option("user-confirmed", Text, true),
        ],
        "secure-mesh approval request"
        | "secure-mesh approval fanout"
        | "secure-mesh approval inbox"
        | "secure-mesh approval adapter-capability" => &[
            value_option("pending-operation-id", Text, false),
            value_option("decision", Text, false),
        ],
        "secure-mesh approval respond" => &[
            value_option("pending-operation-id", Text, true),
            value_option("decision", Text, true),
            value_option("responding-endpoint-id", Text, true),
            value_option("response-nonce", Text, true),
        ],
        _ => &[],
    };
    options.to_vec()
}

fn constraints_for_route(path: &str) -> &'static [ConstraintAuthority] {
    use OptionConstraintKind::{ConditionalRequired, MutuallyExclusive, OneOf};
    match path {
        "update status" | "update check" | "update download" | "update verify" | "update apply" => {
            &[
                ConstraintAuthority {
                    kind: MutuallyExclusive,
                    members: &["source-path", "source"],
                    condition_option: None,
                    condition_value: None,
                    required_option: None,
                },
                ConstraintAuthority {
                    kind: MutuallyExclusive,
                    members: &["source-path", "repo"],
                    condition_option: None,
                    condition_value: None,
                    required_option: None,
                },
            ]
        }
        "snapshots profiles list" | "snapshots profiles get" | "snapshots profiles import" => {
            &[ConstraintAuthority {
                kind: MutuallyExclusive,
                members: &["profile", "profile-json", "profile-file"],
                condition_option: None,
                condition_value: None,
                required_option: None,
            }]
        }
        "snapshots archive jobs preview" | "snapshots archive jobs create" => {
            &[ConstraintAuthority {
                kind: ConditionalRequired,
                members: &[],
                condition_option: Some("selection-mode"),
                condition_value: Some("exact-keyword"),
                required_option: Some("query"),
            }]
        }
        "snapshots archive verify" => &[ConstraintAuthority {
            kind: OneOf,
            members: &["profile", "collection-path"],
            condition_option: None,
            condition_value: None,
            required_option: None,
        }],
        "collaboration install plan"
        | "collaboration install apply"
        | "collaboration install cancel" => &[ConstraintAuthority {
            kind: MutuallyExclusive,
            members: &["github-url", "plan-id"],
            condition_option: None,
            condition_value: None,
            required_option: None,
        }],
        "skill usage report" => &[ConstraintAuthority {
            kind: MutuallyExclusive,
            members: &["days", "from"],
            condition_option: None,
            condition_value: None,
            required_option: None,
        }],
        _ => &[],
    }
}

// ---------------------------------------------------------------------------
// The model-selection document the client reads
// ---------------------------------------------------------------------------
//
// `targets model-selection <target>` composes the one document the desktop
// projection renders. Four dimensions travel in it — support, availability,
// credentials and the per-scope execution outcome — and this section checks
// that they arrive separately, that a name nothing declares is never promoted
// into a support claim, and that a host which composes no policy owner states
// `undetermined` instead of `allowed`.
//
// It is not one of the generated bridge families: it has no manifest entry and
// no generated artifact, so the file below is only a contract while a
// validating owner parses the producer's own output against it. That owner is
// this suite, registered in the module catalog with the schema as an input.

/// The declared document contract for the selection matrix.
const MODEL_SELECTION_SCHEMA_PATH: &str = "schemas/client_bridge/model_selection.json";

/// The producer capture the desktop projection consumes in its own suite. It
/// is pinned below to the output of the producer pipeline, so the Dart side
/// reads a real document instead of a literal its own author typed.
const MODEL_SELECTION_FIXTURE_PATH: &str =
    "apps/desktop/test/fixtures/model_selection/producer_document.json";

/// The fixture observation: one model the catalogue declares and one name
/// nothing declares, both reported by a live source at a stated time.
const MODEL_SELECTION_AGENT: &str = "kimi-code";
const MODEL_SELECTION_DECLARED_MODEL: &str = "k3-256k";
const MODEL_SELECTION_UNDECLARED_MODEL: &str = "mystery-preview";
const MODEL_SELECTION_OBSERVED_AT_UNIX_MS: u64 = 1_726_000_000_000;
const MODEL_SELECTION_GENERATION: &str = "generation-fixture-1";

/// An Agent this host does not declare: the probe refuses to look at it, and
/// the verb states that refusal instead of a document.
const MODEL_SELECTION_UNDECLARED_AGENT: &str = "not-a-declared-agent-2718";

/// The Agent the CLI case seeds an archived observation for. The archive is
/// the observation a previous scan persisted; it is used exactly as a live
/// probe's result would be.
const MODEL_SELECTION_CLI_AGENT: &str = "trae-agent";

fn repository_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

/// The declaration index the fixture observation is joined against.
fn model_selection_fixture_registry() -> licoup_model_catalog::RegistrySnapshot {
    licoup_model_catalog::RegistrySnapshot::from_catalog_with_wrappers(
        json!({
            "models": { "moonshotai/kimi-k3": { "name": "Kimi K3" } },
            "providers": {
                "kimi-for-coding": {
                    "name": "Kimi Code",
                    "models": {
                        "k3-256k": {
                            "name": "Kimi Code K3 256K",
                            "base_model": "moonshotai/kimi-k3"
                        }
                    }
                }
            }
        }),
        Vec::<String>::new(),
    )
    .expect("synthetic declaration index")
}

/// What a live source reported for the fixture Agent.
fn model_selection_fixture_report() -> licoup_model_catalog::SelectionReport {
    let registry = model_selection_fixture_registry();
    let observed = || licoup_model_catalog::ObservedAvailability::Observed {
        at_unix_ms: MODEL_SELECTION_OBSERVED_AT_UNIX_MS,
        sources: vec!["kimi-code-config".to_owned()],
    };
    licoup_model_catalog::SelectionReport {
        target: MODEL_SELECTION_AGENT.to_owned(),
        generation: Some(MODEL_SELECTION_GENERATION.to_owned()),
        observed_at_unix_ms: MODEL_SELECTION_OBSERVED_AT_UNIX_MS,
        entries: vec![
            licoup_model_catalog::selection_facts(
                &registry,
                MODEL_SELECTION_DECLARED_MODEL,
                Some("kimi-for-coding"),
                Some(MODEL_SELECTION_AGENT),
                observed(),
                vec!["kimi-for-coding".to_owned()],
            ),
            licoup_model_catalog::selection_facts(
                &registry,
                MODEL_SELECTION_UNDECLARED_MODEL,
                None,
                Some(MODEL_SELECTION_AGENT),
                observed(),
                Vec::new(),
            ),
        ],
    }
}

/// The document the producer pipeline renders, through the same functions the
/// domain composes and the same fail-closed ports a host with no owner keeps.
fn model_selection_fixture_document() -> Value {
    licoup_model_catalog::selection_matrix_document(&licoup_model_catalog::selection_matrix(
        &licoup_model_catalog::ModelCatalogPort::unavailable(),
        &licoup_model_catalog::SelectionMatrixPort::unavailable(),
        true,
        &model_selection_fixture_report(),
        &json!({}),
    ))
}

/// The same document for a host whose inventory answer is `false`: the Agent
/// is not declared here, whatever a source reported.
fn model_selection_document_for_undeclared_agent() -> Value {
    licoup_model_catalog::selection_matrix_document(&licoup_model_catalog::selection_matrix(
        &licoup_model_catalog::ModelCatalogPort::unavailable(),
        &licoup_model_catalog::SelectionMatrixPort::unavailable(),
        false,
        &model_selection_fixture_report(),
        &json!({}),
    ))
}

/// Persist one archived observation, the way a completed scan persists it.
fn seed_archived_model_catalog(store_root: &Path, target: &str, models: &[&str]) {
    let store =
        licoup_native::platform::client_state::ClientStateStore::new(store_root.to_path_buf())
            .expect("client-state store");
    let catalog = json!({
        "schemaVersion": 1,
        "status": "available",
        "sources": ["config"],
        "models": models
            .iter()
            .map(|name| json!({ "name": name, "sources": ["config"] }))
            .collect::<Vec<_>>(),
    });
    store
        .write_target_routes(&[licoup_client_state::TargetRouteRecord {
            schema_version: licoup_client_state::TARGET_DISCOVERY_CACHE_SCHEMA.to_owned(),
            target: target.to_owned(),
            binary_path: None,
            config_path: None,
            scan_source: "cache".to_owned(),
            runtime_ready: false,
            cached_at_epoch_seconds: 1_726_000_000,
            extension: serde_json::Map::from_iter([("modelCatalog".to_owned(), catalog)]),
        }])
        .expect("archived observation");
}

/// The declared contract, read from the schema file itself so the producer is
/// checked against the document rather than against a copy of it.
struct ModelSelectionContract {
    schema_version: u64,
    scopes: Vec<String>,
    maximum_entries: u64,
    maximum_reason_bytes: u64,
    support_states: BTreeSet<String>,
    availability_states: BTreeSet<String>,
    credential_states: BTreeSet<String>,
    scope_states: BTreeSet<String>,
    entry_fields: BTreeSet<String>,
    document_example: Value,
}

impl ModelSelectionContract {
    fn read() -> Self {
        let path = repository_path(MODEL_SELECTION_SCHEMA_PATH);
        let schema: Value = serde_json::from_str(
            &fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("missing {}: {error}", path.display())),
        )
        .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()));
        // The contract names the producer and the client paths it describes.
        // A contract that names a module nobody wires describes nothing.
        for (label, relative) in [
            ("producer.composedBy", &schema["producer"]["composedBy"]),
            ("producer.validatedBy", &schema["producer"]["validatedBy"]),
            ("consumer.contract", &schema["consumer"]["contract"]),
            ("consumer.projection", &schema["consumer"]["projection"]),
        ] {
            let relative = relative
                .as_str()
                .unwrap_or_else(|| panic!("{label} must name a path"));
            assert!(
                repository_path(relative).exists(),
                "{label} names a missing path: {relative}"
            );
        }
        let states = |dimension: &str, key: &str| -> BTreeSet<String> {
            schema["dimensions"][dimension][key]
                .as_object()
                .unwrap_or_else(|| panic!("{dimension}.{key} must declare its states"))
                .keys()
                .cloned()
                .collect()
        };
        let entry_fields = schema["document"]["entries"]
            .as_array()
            .expect("the contract shows its own document")
            .iter()
            .flat_map(|entry| {
                entry
                    .as_object()
                    .expect("a contract entry is an object")
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect();
        Self {
            schema_version: schema["version"]
                .as_u64()
                .expect("the contract declares a version"),
            scopes: schema["scopes"]
                .as_array()
                .expect("the contract declares its scopes")
                .iter()
                .map(|scope| scope.as_str().expect("a scope is a string").to_owned())
                .collect(),
            maximum_entries: schema["maximumEntries"]
                .as_u64()
                .expect("the contract bounds its entries"),
            maximum_reason_bytes: schema["maximumReasonBytes"]
                .as_u64()
                .expect("the contract bounds its reasons"),
            support_states: states("support", "states"),
            availability_states: states("availability", "states"),
            credential_states: states("credentials", "states"),
            scope_states: states("execution", "scopeStates"),
            entry_fields,
            document_example: schema["document"].clone(),
        }
    }

    /// One reason is a state's own explanation: it is never absent and never
    /// longer than the contract allows, because a state without a reason
    /// cannot be inspected.
    fn reason(&self, value: &Value, label: &str) -> String {
        let reason = value["reason"]
            .as_str()
            .unwrap_or_else(|| panic!("{label} must carry a reason"));
        assert!(!reason.trim().is_empty(), "{label} carries an empty reason");
        assert!(
            reason.len() <= self.maximum_reason_bytes as usize,
            "{label} reason is longer than the contract allows"
        );
        reason.to_owned()
    }

    fn validate(&self, document: &Value) {
        assert_eq!(document["schemaVersion"], json!(self.schema_version));
        assert_eq!(document["scopes"], json!(self.scopes));
        assert!(
            document["agent"]
                .as_str()
                .is_some_and(|agent| !agent.trim().is_empty()),
            "a document names the Agent it describes"
        );
        let entries = document["entries"]
            .as_array()
            .expect("a document carries its entries");
        assert!(entries.len() as u64 <= self.maximum_entries);
        for entry in entries {
            let entry = entry
                .as_object()
                .expect("an entry is an object, not a collapsed label");
            for field in entry.keys() {
                assert!(
                    self.entry_fields.contains(field),
                    "the producer emits an undeclared entry field: {field}"
                );
            }
            // The contract's own document places every entry under the Agent
            // the document is about; an entry that names its model there puts
            // the row under the wrong heading.
            assert_eq!(
                entry.get("agent"),
                document.get("agent"),
                "every entry names the Agent the document describes"
            );
            for (dimension, states) in [
                ("support", &self.support_states),
                ("availability", &self.availability_states),
                ("credentials", &self.credential_states),
            ] {
                let value = entry
                    .get(dimension)
                    .filter(|value| value.is_object())
                    .unwrap_or_else(|| panic!("{dimension} is its own object"));
                let state = value["state"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{dimension} states its state"));
                assert!(
                    states.contains(state),
                    "{dimension} state {state} is not declared by the contract"
                );
                self.reason(value, dimension);
            }
            // A dimension is never promoted into a claim the evidence does not
            // support: no declared identity is not a support claim, and no
            // observation is not an observed state.
            if entry["canonicalId"].is_null() {
                assert_ne!(
                    entry["support"]["state"], "supported",
                    "a name nothing declares is never promoted to supported"
                );
            }
            if entry["availability"]["observedAtUnixMs"].is_null() {
                assert_ne!(
                    entry["availability"]["state"], "observed",
                    "an observation nobody took is never rendered as observed"
                );
            }
            let scopes = entry["scopes"]
                .as_array()
                .expect("both scopes are always present");
            assert_eq!(scopes.len(), self.scopes.len());
            for (position, scope) in scopes.iter().enumerate() {
                assert_eq!(scope["scope"], json!(self.scopes[position]));
                let state = scope["state"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a scope states its outcome"));
                assert!(
                    self.scope_states.contains(state),
                    "scope state {state} is not declared by the contract"
                );
                self.reason(scope, "execution");
            }
        }
    }
}

/// Run the verb the way a client does: the real binary, on a synthetic home
/// and data root, composing its own ports at startup. A test process installs
/// no catalogue port, so the in-process dispatch would answer every probe
/// fail-closed; this is the composition a client actually reaches.
fn run_targets_model_selection(root: &Path, agent: &str) -> Output {
    let cli = SyntheticCliHome::new(Some(root));
    let output = cli
        .command()
        .args(["targets", "model-selection", agent])
        .output()
        .expect("spawn the native CLI");
    let _ = fs::remove_dir_all(&cli.home);
    output
}

/// The verb reaches the client, and the four dimensions arrive separately with
/// their own states and reason codes.
#[test]
fn targets_model_selection_states_every_dimension_separately() {
    let root = temporary_directory("licoup-model-selection-cli");
    seed_archived_model_catalog(
        &root.join("client-state"),
        MODEL_SELECTION_CLI_AGENT,
        &["k3-256k", "mystery-preview"],
    );
    let output = run_targets_model_selection(&root, MODEL_SELECTION_CLI_AGENT);
    let _ = fs::remove_dir_all(&root);
    assert!(
        output.status.success(),
        "the selection verb must compose a document through the declared ports: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout)
        .expect("targets model-selection returns the document as JSON");
    ModelSelectionContract::read().validate(&document);

    assert_eq!(document["schemaVersion"], 1);
    assert_eq!(document["agent"], MODEL_SELECTION_CLI_AGENT);
    assert_eq!(document["scopes"], json!(["direct", "workflow"]));
    let entries = document["entries"]
        .as_array()
        .expect("the document carries its entries");
    assert!(
        !entries.is_empty(),
        "a seeded observation must reach the document: {document}"
    );
    for entry in entries {
        // Four dimensions, four separate objects, each with its own state and
        // its own reason: nothing on the entry is a collapsed readiness label.
        for collapsed in ["state", "ready", "readiness", "available"] {
            assert!(
                entry.get(collapsed).is_none(),
                "an entry must not collapse its dimensions into {collapsed}: {entry}"
            );
        }
        let support = &entry["support"];
        let availability = &entry["availability"];
        let credentials = &entry["credentials"];
        assert!(
            matches!(
                support["state"].as_str(),
                Some("supported" | "unsupported" | "unknown")
            ),
            "{entry}"
        );
        // A name this host declares no identity for keeps unknown support; one
        // whose identity resolves is supported. Neither is inferred from the
        // other dimension.
        if entry["canonicalId"].is_null() {
            assert_eq!(support["state"], "unknown", "{entry}");
            assert_eq!(support["reason"], "observed_without_declared_identity");
        } else {
            assert_eq!(support["state"], "supported", "{entry}");
            assert_eq!(support["reason"], "declared_identity");
        }
        assert!(matches!(
            availability["state"].as_str(),
            Some("observed" | "unobserved")
        ));
        // The observation this report was built from is the document's own.
        assert_eq!(availability["state"], "observed");
        assert_eq!(
            availability["observedAtUnixMs"],
            document["observedAtUnixMs"]
        );
        // No owner publishes a credential mapping, so every provider stays
        // unknown — and never absent, which would be a claim.
        assert_eq!(credentials["state"], "unknown");
        assert!(
            credentials["reason"]
                .as_str()
                .is_some_and(|reason| matches!(
                    reason,
                    "provider_not_recorded" | "provider_credential_unknown"
                ))
        );
        // Support, availability and credentials are three statements, and the
        // entry keeps them apart rather than deriving one from another.
        assert_ne!(support["state"], availability["state"]);
        assert_ne!(availability["state"], credentials["state"]);

        // Both scopes are always present and decided separately; the composed
        // policy owner answers neither, so both stay undetermined.
        let scopes = entry["scopes"]
            .as_array()
            .expect("both scopes are always present");
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0]["scope"], "direct");
        assert_eq!(scopes[1]["scope"], "workflow");
        for scope in scopes {
            assert_eq!(scope["state"], "undetermined", "{entry}");
            assert_eq!(scope["reason"], "selection_policy_owner_absent");
        }
    }
}

/// An Agent this host does not declare never becomes a document with a support
/// claim: the probe refuses to look, and "could not look" is not "nothing
/// there".
#[test]
fn targets_model_selection_refuses_an_agent_the_host_does_not_declare() {
    let root = temporary_directory("licoup-model-selection-undeclared");
    let output = run_targets_model_selection(&root, MODEL_SELECTION_UNDECLARED_AGENT);
    let _ = fs::remove_dir_all(&root);
    assert!(
        !output.status.success(),
        "an Agent this host does not declare has no selection document"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("schemaVersion"),
        "a refusal is not a document: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unsupported target adapter")
            && stderr.contains(MODEL_SELECTION_UNDECLARED_AGENT),
        "the refusal names the unavailable Agent: {stderr}"
    );
}

/// The statements the probe refuses to make are still made honestly by the
/// document composer: an Agent the inventory does not declare keeps unknown
/// support, and a host with no policy owner reports every scope undetermined
/// instead of allowed.
#[test]
fn an_undeclared_agent_and_an_unanswered_policy_owner_are_never_promoted() {
    let contract = ModelSelectionContract::read();
    let document = model_selection_document_for_undeclared_agent();
    contract.validate(&document);
    let entries = document["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 2);
    for entry in entries {
        assert_eq!(entry["support"]["state"], "unknown", "{entry}");
        assert_eq!(entry["support"]["reason"], "agent_not_declared_on_host");
        // The observation is still reported as what it is; the Agent's missing
        // declaration does not erase it.
        assert_eq!(entry["availability"]["state"], "observed");
        assert_eq!(entry["credentials"]["state"], "unknown");
        for scope in entry["scopes"].as_array().expect("scopes") {
            assert_eq!(scope["state"], "undetermined", "{entry}");
            assert_eq!(scope["reason"], "selection_policy_owner_absent");
            assert_ne!(scope["state"], "allowed");
        }
    }

    // A declared Agent keeps the same four dimensions with four different
    // states in one entry: supported identity, observed availability, unknown
    // credentials and an undetermined execution outcome.
    let declared = model_selection_fixture_document();
    contract.validate(&declared);
    let entry = &declared["entries"][0];
    assert_eq!(entry["support"]["state"], "supported");
    assert_eq!(entry["support"]["reason"], "declared_identity");
    assert_eq!(entry["availability"]["state"], "observed");
    assert_eq!(entry["availability"]["reason"], "observed_by_live_source");
    assert_eq!(entry["credentials"]["state"], "unknown");
    assert_eq!(
        entry["credentials"]["reason"],
        "provider_credential_unknown"
    );
    assert_eq!(entry["scopes"][0]["state"], "undetermined");
    assert_ne!(entry["scopes"][0]["state"], "allowed");
}

/// The hand-written contract obeys itself: its own example is a document of
/// the revision, scopes and states it declares. A contract whose example
/// states something it does not declare is not a contract.
#[test]
fn the_declared_selection_contract_obeys_its_own_example() {
    let contract = ModelSelectionContract::read();
    contract.validate(&contract.document_example);
}

/// The desktop projection reads a real value: this fixture is the producer's
/// own document, and the Dart suite consumes this file instead of a literal
/// its author typed.
#[test]
fn the_producer_document_is_pinned_for_the_desktop_projection() {
    let document = model_selection_fixture_document();
    ModelSelectionContract::read().validate(&document);
    let path = repository_path(MODEL_SELECTION_FIXTURE_PATH);
    let pinned: Value = serde_json::from_str(
        &fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("missing {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()));
    assert_eq!(
        pinned,
        document,
        "the pinned client fixture is stale; the producer now renders:\n{}",
        serde_json::to_string_pretty(&document).expect("render the producer document")
    );
}

// ---------------------------------------------------------------------------
// Archive recovery through the real binary on the selected data home
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod recovery_cli {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const LEASE_ACTION: &str = "LICOUP_TEST_CLI_LEASE_ACTION";
    const SNAPSHOT_ROOT_KEY: &str = "conversationSnapshotRoot";
    const UNRELATED_USER_PATH: &str = "/tmp/unrelated-user-project";

    fn recovery_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let fixture = temporary_directory(label)
            .canonicalize()
            .expect("canonical recovery fixture");
        let home = fixture.join("home");
        let root = fixture.join("root");
        fs::create_dir_all(&home).expect("fixture home");
        fs::create_dir_all(&root).expect("fixture root");
        (fixture, home, root)
    }

    fn locator_path(home: &Path) -> PathBuf {
        licoup_foundation::platform::paths::data_home_locator_path_for(
            licoup_foundation::platform::paths::DataHomePlatform::current(),
            home,
            Some(home.join("AppData/Roaming").into_os_string()),
            Some(home.join(".config").into_os_string()),
        )
    }

    fn save_locator(home: &Path, root: &Path) {
        let locator = locator_path(home);
        fs::create_dir_all(locator.parent().expect("locator parent")).expect("locator directory");
        fs::set_permissions(
            locator.parent().expect("locator parent"),
            fs::Permissions::from_mode(0o700),
        )
        .expect("locator directory permissions");
        fs::write(&locator, format!("{}\n", root.display())).expect("locator");
        fs::set_permissions(&locator, fs::Permissions::from_mode(0o600))
            .expect("locator permissions");
    }

    fn arrange_root(root: &Path) {
        let store =
            licoup_native::platform::client_state::ClientStateStore::new(root.join("client-state"))
                .expect("client-state owner");
        store
            .write_collection(
                "settings",
                json!({
                    SNAPSHOT_ROOT_KEY: root.join("snapshots").display().to_string(),
                    "userProjectPath": UNRELATED_USER_PATH,
                }),
            )
            .expect("arrange settings");
        fs::write(root.join("member.bin"), b"member-canary").expect("arrange member");
    }

    fn run_backup(
        home: &Path,
        args: &[String],
        current_dir: Option<&Path>,
        environment: &[(&str, &str)],
    ) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_licoup-cli"));
        command
            .args(args)
            .env("HOME", home)
            .env_remove("LICOUP_HOME")
            .env_remove("LICOUP_PORTABLE_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("RUST_LOG")
            .env_remove("RUST_BACKTRACE");
        for (key, value) in environment {
            command.env(key, value);
        }
        if let Some(directory) = current_dir {
            command.current_dir(directory);
        }
        command
            .output()
            .expect("the real licoup binary must be runnable")
    }

    fn export(home: &Path, root: &Path, archive: &Path) -> Value {
        let output = run_backup(
            home,
            &[
                "backup".to_owned(),
                "export".to_owned(),
                archive.display().to_string(),
                "--data-root".to_owned(),
                root.display().to_string(),
                "--writers-stopped".to_owned(),
            ],
            None,
            &[],
        );
        assert!(
            output.status.success(),
            "export must succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("export reports JSON")
    }

    #[test]
    fn backup_export_and_import_round_trip_the_selected_home() {
        let (fixture, home, root) = recovery_fixture("licoup-backup-round-trip");
        save_locator(&home, &root);
        arrange_root(&root);

        for name in ["home.zip", "home.tar.gz"] {
            let archive = fixture.join(name);
            let output = run_backup(
                &home,
                &[
                    "backup".to_owned(),
                    "export".to_owned(),
                    archive.display().to_string(),
                    "--writers-stopped".to_owned(),
                ],
                None,
                &[],
            );
            assert!(
                output.status.success(),
                "export {name}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let exported: Value = serde_json::from_slice(&output.stdout).expect("export JSON");
            assert_eq!(exported["status"], "exported");
            assert_eq!(
                exported["sourceHome"].as_str(),
                Some(root.display().to_string().as_str()),
                "the default export selects the saved data home"
            );
            assert_eq!(
                exported["container"],
                if name.ends_with(".zip") {
                    "zip"
                } else {
                    "tar.gz"
                }
            );
            assert_eq!(exported["coverage"], "limited");
            assert_eq!(
                exported["limitations"][0]["domain"],
                "gateway-credential-custody"
            );

            let target = fixture.join(format!("{name}-target"));
            let output = run_backup(
                &home,
                &[
                    "backup".to_owned(),
                    "import".to_owned(),
                    archive.display().to_string(),
                    "--target-root".to_owned(),
                    target.display().to_string(),
                ],
                None,
                &[],
            );
            assert!(
                output.status.success(),
                "import {name}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let imported: Value = serde_json::from_slice(&output.stdout).expect("import JSON");
            assert_eq!(imported["status"], "imported");
            assert_eq!(imported["relocated"], true);
            assert_eq!(
                imported["sourceHome"].as_str(),
                Some(root.display().to_string().as_str())
            );
            assert_eq!(
                fs::read(target.join("member.bin")).expect("member"),
                b"member-canary"
            );
            let restored = licoup_native::platform::client_state::ClientStateStore::new(
                target.join("client-state"),
            )
            .expect("client-state owner")
            .read_collection("settings")
            .expect("settings read");
            assert_eq!(
                restored[SNAPSHOT_ROOT_KEY].as_str(),
                Some(target.join("snapshots").display().to_string().as_str()),
                "the owned reference follows the restored home"
            );
            assert_eq!(
                restored["userProjectPath"].as_str(),
                Some(UNRELATED_USER_PATH)
            );
        }
        let _ = fs::remove_dir_all(fixture);
    }

    #[test]
    fn backup_export_resolves_environment_alias_explicit_and_relative_roots() {
        let (fixture, home, root) = recovery_fixture("licoup-backup-resolution");
        let other = fixture.join("other-root");
        fs::create_dir_all(&other).expect("other root");
        let relative_root = fixture.join("relative-root");
        fs::create_dir_all(&relative_root).expect("relative root");

        let archive = fixture.join("environment.zip");
        let exported = export_with(
            &home,
            &archive,
            &[("LICOUP_HOME", root.display().to_string().as_str())],
        );
        assert_eq!(
            exported["sourceHome"].as_str(),
            Some(root.display().to_string().as_str()),
            "LICOUP_HOME selects the data home"
        );

        let archive = fixture.join("alias.zip");
        let exported = export_with(
            &home,
            &archive,
            &[("LICOUP_PORTABLE_DIR", root.display().to_string().as_str())],
        );
        assert_eq!(
            exported["sourceHome"].as_str(),
            Some(root.display().to_string().as_str()),
            "the published alias selects the data home"
        );

        let archive = fixture.join("explicit.zip");
        let exported = export(&home, &other, &archive);
        assert_eq!(
            exported["sourceHome"].as_str(),
            Some(other.display().to_string().as_str()),
            "an explicit absolute root selects that root"
        );

        let output = run_backup(
            &home,
            &[
                "backup".to_owned(),
                "export".to_owned(),
                "relative.zip".to_owned(),
                "--data-root".to_owned(),
                "relative-root".to_owned(),
                "--writers-stopped".to_owned(),
            ],
            Some(&fixture),
            &[],
        );
        assert!(
            output.status.success(),
            "relative export: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let exported: Value = serde_json::from_slice(&output.stdout).expect("export JSON");
        assert_eq!(
            exported["sourceHome"].as_str(),
            Some(relative_root.display().to_string().as_str()),
            "a relative root resolves against the process working directory"
        );
        assert!(fixture.join("relative.zip").is_file());
        let _ = fs::remove_dir_all(fixture);
    }

    fn export_with(home: &Path, archive: &Path, environment: &[(&str, &str)]) -> Value {
        let output = run_backup(
            home,
            &[
                "backup".to_owned(),
                "export".to_owned(),
                archive.display().to_string(),
                "--writers-stopped".to_owned(),
            ],
            None,
            environment,
        );
        assert!(
            output.status.success(),
            "export must succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("export JSON")
    }

    #[test]
    fn backup_commands_refuse_unadmissible_writers_and_destinations() {
        let (fixture, home, root) = recovery_fixture("licoup-backup-refusals");
        let archive = fixture.join("refused.zip");

        // The caller's stopped-writer statement is required.
        let output = run_backup(
            &home,
            &[
                "backup".to_owned(),
                "export".to_owned(),
                archive.display().to_string(),
                "--data-root".to_owned(),
                root.display().to_string(),
            ],
            None,
            &[],
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("archive_writers_running"),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!archive.exists(), "a refused export publishes nothing");

        // A destination inside the captured root is refused.
        let inside = root.join("inside.zip");
        let output = run_backup(
            &home,
            &[
                "backup".to_owned(),
                "export".to_owned(),
                inside.display().to_string(),
                "--data-root".to_owned(),
                root.display().to_string(),
                "--writers-stopped".to_owned(),
            ],
            None,
            &[],
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("archive_path_inside_data_root"),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!inside.exists());

        // A non-empty import target is refused and left untouched.
        let archive = fixture.join("complete.zip");
        export(&home, &root, &archive);
        let occupied = fixture.join("occupied");
        fs::create_dir_all(&occupied).expect("occupied target");
        fs::write(occupied.join("keep.txt"), b"keep").expect("occupied member");
        let output = run_backup(
            &home,
            &[
                "backup".to_owned(),
                "import".to_owned(),
                archive.display().to_string(),
                "--target-root".to_owned(),
                occupied.display().to_string(),
            ],
            None,
            &[],
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("archive_target_not_empty"),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(occupied.join("keep.txt")).unwrap(), b"keep");
        assert_eq!(
            fs::read_dir(&occupied)
                .expect("occupied target")
                .filter_map(Result::ok)
                .count(),
            1,
            "a refused import publishes no destination member"
        );
        let _ = fs::remove_dir_all(fixture);
    }

    #[test]
    fn backup_refuses_an_active_data_home_writer_without_stopping_it() {
        let (fixture, home, root) = recovery_fixture("licoup-backup-lease");
        save_locator(&home, &root);
        arrange_root(&root);

        let mut writer = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "recovery_cli::data_home_lease_holder_helper",
                "--nocapture",
            ])
            .env(LEASE_ACTION, "hold")
            .env("HOME", &home)
            .env_remove("LICOUP_HOME")
            .env_remove("LICOUP_PORTABLE_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the writer helper must start");
        let mut stdout = BufReader::new(writer.stdout.take().expect("helper stdout"));
        let mut line = String::new();
        loop {
            line.clear();
            let read = stdout.read_line(&mut line).expect("helper line");
            assert_ne!(
                read, 0,
                "the writer helper exited before acquiring its lease"
            );
            if line.trim() == "DATA_HOME_LEASE_READY" {
                break;
            }
        }

        let archive = fixture.join("blocked.zip");
        let output = run_backup(
            &home,
            &[
                "backup".to_owned(),
                "export".to_owned(),
                archive.display().to_string(),
                "--writers-stopped".to_owned(),
            ],
            None,
            &[],
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("backup_writers_running"),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!archive.exists());
        assert!(
            writer.try_wait().expect("writer state").is_none(),
            "the active writer must not be stopped"
        );

        writer
            .stdin
            .as_mut()
            .expect("helper stdin")
            .write_all(b"release\n")
            .expect("release the writer");
        assert!(writer.wait().expect("writer exits").success());

        let output = run_backup(
            &home,
            &[
                "backup".to_owned(),
                "export".to_owned(),
                archive.display().to_string(),
                "--writers-stopped".to_owned(),
            ],
            None,
            &[],
        );
        assert!(
            output.status.success(),
            "export after the writer drained: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(archive.is_file());
        let _ = fs::remove_dir_all(fixture);
    }

    /// Cross-process fixture: holds the shared process lease until the parent says
    /// `release`. The `--exact` filter runs only this function.
    #[test]
    fn data_home_lease_holder_helper() {
        let Ok(action) = std::env::var(LEASE_ACTION) else {
            return;
        };
        assert_eq!(action, "hold");
        let _lease =
            licoup_foundation::platform::data_home_access::acquire_process_data_home_access()
                .expect("the helper acquires the shared process lease");
        println!("DATA_HOME_LEASE_READY");
        std::io::stdout().flush().expect("helper stdout flush");
        let mut release = String::new();
        std::io::stdin()
            .read_line(&mut release)
            .expect("helper release line");
        assert_eq!(release.trim(), "release");
    }
}

fn counted_unknown_route(total: usize, canary: &str, oversized_first: bool) -> Vec<String> {
    assert!(total > 0);
    let first = if oversized_first {
        exact_byte_argument(canary, MAX_CLI_ARGUMENT_BYTES + 1)
    } else {
        canary.to_string()
    };
    let mut args = Vec::with_capacity(total);
    args.push(first);
    args.resize(total, "x".to_string());
    args
}

fn exact_byte_argument(canary: &str, bytes: usize) -> String {
    assert!(canary.is_ascii());
    assert!(canary.len() <= bytes);
    format!("{canary}{}", "x".repeat(bytes - canary.len()))
}

fn exact_multibyte_argument(canary: &str, bytes: usize) -> String {
    assert!(canary.is_ascii());
    assert!(canary.len() <= bytes);
    let mut value = canary.to_string();
    let multibyte_count = (bytes - value.len()) / "界".len();
    value.push_str(&"界".repeat(multibyte_count));
    value.push_str(&"x".repeat(bytes - value.len()));
    assert_eq!(value.len(), bytes);
    value
}

fn oversized_json_payload(canary: &str) -> String {
    format!(
        r#"{{"items":[{{"value":"{canary}{}"}}]}}"#,
        "x".repeat(MAX_CLI_ARGUMENT_BYTES)
    )
}

fn state_count_overflow(payload: &str, canary: &str) -> Vec<String> {
    let mut args = strings(["state", "set", "settings", payload]);
    args.resize(MAX_CLI_ARGUMENT_COUNT + 1, canary.to_string());
    args
}

fn assert_state_unchanged(baseline: &Value, rejected_case: &str) {
    assert_eq!(
        state_get(),
        *baseline,
        "{rejected_case} must not mutate portable state"
    );
}

fn state_set(value: &Value) -> Value {
    let payload = value.to_string();
    match execute_cli(strings(["state", "set", "settings", payload.as_str()]))
        .expect("valid state set must succeed")
    {
        CliExecution::Json(value) => value["document"].clone(),
        other => panic!("valid state set returned {other:?}"),
    }
}

fn state_get() -> Value {
    match execute_cli(strings(["state", "get", "settings"])).expect("valid state get must succeed")
    {
        CliExecution::Json(value) => value["document"].clone(),
        other => panic!("valid state get returned {other:?}"),
    }
}

fn strings<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    values.into_iter().map(str::to_owned).collect()
}

fn cli_environment_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn temporary_directory(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{label}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).expect("create isolated portable state root");
    path
}

struct PortableDataOverride {
    previous: Option<PathBuf>,
    root: PathBuf,
}

/// Every real subprocess gets a synthetic user home as well as a data root.
/// This keeps admission/RPC fixtures away from the user's locator and services.
struct SyntheticCliHome {
    home: PathBuf,
    root: PathBuf,
    stopped: std::cell::Cell<bool>,
}

impl SyntheticCliHome {
    fn new(root: Option<&Path>) -> Self {
        let home = temporary_directory("licoup-cli-home");
        let root = root
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join("data"));
        fs::create_dir_all(&root).expect("synthetic CLI data root");
        Self {
            home,
            root,
            stopped: std::cell::Cell::new(false),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_licoup-cli"));
        command
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data-dir"))
            .env("APPDATA", self.home.join("appdata"))
            .env("LOCALAPPDATA", self.home.join("local-appdata"))
            .env("LICOUP_HOME", &self.root)
            .env_remove("LICOUP_CLIENT_PID")
            .env("LICO_MOBILE_RELAY_NATIVE_SECRET_STORE", "disabled")
            .env_remove("RUST_LOG")
            .env_remove("RUST_BACKTRACE");
        command
    }

    fn stop(&self) -> std::io::Result<()> {
        if self.stopped.get() {
            return Ok(());
        }
        let output = self
            .command()
            .args(["rpc", "conversation-host", "--stop"])
            .output()?;
        if output.status.success() {
            self.stopped.set(true);
            Ok(())
        } else {
            Err(std::io::Error::other(
                "synthetic conversation host did not stop",
            ))
        }
    }
}

impl Drop for SyntheticCliHome {
    fn drop(&mut self) {
        if self.stop().is_ok() {
            let _ = fs::remove_dir_all(&self.home);
        }
    }
}

impl PortableDataOverride {
    fn set(root: &Path) -> Self {
        let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
            root.to_path_buf(),
        ));
        Self {
            previous,
            root: root.to_path_buf(),
        }
    }
}

impl Drop for PortableDataOverride {
    fn drop(&mut self) {
        licoup_foundation::platform::paths::set_portable_data_dir_override(self.previous.take());
        let _ = fs::remove_dir_all(&self.root);
    }
}

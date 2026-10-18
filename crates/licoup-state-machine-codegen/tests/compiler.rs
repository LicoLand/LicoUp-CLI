use licoup_state_machine_codegen::compile_file;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_directory(test_name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "licoup-state-machine-codegen-{}-{test_name}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("temporary test directory should be created");
    path
}

fn compile(test_name: &str, document: &str) -> Result<String, String> {
    let directory = temporary_directory(test_name);
    let input = directory.join("machine.json");
    let output = directory.join("machine.rs");
    fs::write(&input, document).expect("fixture should be written");
    let result = compile_file(&input, &output).map_err(|error| error.to_string());
    let generated =
        result.and_then(|()| fs::read_to_string(&output).map_err(|error| error.to_string()));
    fs::remove_dir_all(directory).expect("temporary test directory should be removed");
    generated
}

#[test]
fn configuration_transition_changes_generated_lookup() {
    let first = compile("first", &fixture("idle")).expect("first configuration should compile");
    let second = compile("second", &fixture("done")).expect("second configuration should compile");

    assert!(first.contains("[Some(State::Idle),None,]"));
    assert!(second.contains("[Some(State::Done),None,]"));
    assert_ne!(first, second);
    assert_eq!(run_generated("first-behavior", &first), "idle\n");
    assert_eq!(run_generated("second-behavior", &second), "done\n");
}

#[test]
fn rejects_ambiguous_and_unknown_transitions() {
    let ambiguous = fixture("idle").replace(
        r#"{"from_state":"idle","event":"start","to_state":"idle"}"#,
        r#"{"from_state":"idle","event":"start","to_state":"idle"},
           {"from_state":"idle","event":"start","to_state":"done"}"#,
    );
    assert!(
        compile("ambiguous", &ambiguous)
            .expect_err("ambiguous transition should fail")
            .contains("ambiguous transition")
    );

    let unknown = fixture("idle").replace(r#""event":"start""#, r#""event":"missing""#);
    assert!(
        compile("unknown", &unknown)
            .expect_err("unknown event should fail")
            .contains("unknown transition event")
    );
}

#[test]
fn output_is_deterministic_and_sorted_by_machine_id() {
    let document = r#"{
      "machines": [
        {"id":"z.machine","states":[{"id":"ready"}],"events":["tick"],"initial":"ready","terminal":[],"transitions":[]},
        {"id":"a.machine","states":[{"id":"ready"}],"events":["tick"],"initial":"ready","terminal":[],"transitions":[]}
      ]
    }"#;
    let first = compile("deterministic-one", document).expect("document should compile");
    let second = compile("deterministic-two", document).expect("document should compile");
    assert_eq!(first, second);
    assert!(first.find("a.machine").unwrap() < first.find("z.machine").unwrap());
}

#[test]
fn rejects_terminal_escape_and_generated_identifier_collisions() {
    let terminal_escape = fixture("done").replace(
        r#"{"from_state":"idle","event":"start","to_state":"done"}"#,
        r#"{"from_state":"idle","event":"start","to_state":"done"},
           {"from_state":"done","event":"stop","to_state":"idle"}"#,
    );
    assert!(
        compile("terminal-escape", &terminal_escape)
            .expect_err("terminal escape should fail")
            .contains("escaping transition")
    );

    let collision = fixture("idle").replace(
        r#"{"id":"done"}"#,
        r#"{"id":"done"},{"id":"do-ne"},{"id":"do_ne"}"#,
    );
    assert!(
        compile("collision", &collision)
            .expect_err("Rust name collision should fail")
            .contains("duplicate Rust name")
    );
}

#[test]
fn rejects_empty_documents_and_whitespace_padded_names() {
    assert!(
        compile("empty-document", r#"{"machines":[]}"#)
            .expect_err("empty document should fail")
            .contains("declares no state machines")
    );

    let whitespace = fixture("idle").replace(r#""id":"idle""#, r#""id":" idle""#);
    assert!(
        compile("whitespace", &whitespace)
            .expect_err("whitespace-padded state should fail")
            .contains("whitespace-padded state")
    );
}

fn fixture(target: &str) -> String {
    format!(
        r#"{{
          "machines": [{{
            "id":"demo.machine",
            "states":[{{"id":"idle"}},{{"id":"done"}}],
            "events":["start","stop"],
            "initial":"idle",
            "terminal":["done"],
            "transitions":[{{"from_state":"idle","event":"start","to_state":"{target}"}}]
          }}]
        }}"#
    )
}

fn run_generated(test_name: &str, generated: &str) -> String {
    let directory = temporary_directory(test_name);
    let source = directory.join("main.rs");
    let binary = directory.join("machine-test");
    let generated_without_serde = generated
        .replace(", serde::Serialize, serde::Deserialize", "")
        .lines()
        .filter(|line| !line.trim_start().starts_with("#[serde(rename ="))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        &source,
        format!(
            "{generated_without_serde}\nfn main() {{\n    use demo_machine::{{Event, INITIAL, State}};\n    let next = demo_machine::transition(INITIAL, Event::Start).unwrap();\n    assert!(demo_machine::permits(INITIAL, next));\n    assert!(!demo_machine::permits(INITIAL, State::Done) || next == State::Done);\n    println!(\"{{}}\", next.as_str());\n}}\n"
        ),
    )
    .expect("generated behavior source should be written");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let status = Command::new(rustc)
        .args(["--edition", "2024"])
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .status()
        .expect("rustc should run");
    assert!(status.success(), "generated Rust source should compile");
    let output = Command::new(&binary)
        .output()
        .expect("generated behavior binary should run");
    assert!(output.status.success(), "generated behavior should succeed");
    let stdout = String::from_utf8(output.stdout).expect("behavior output should be UTF-8");
    fs::remove_dir_all(directory).expect("temporary test directory should be removed");
    stdout
}

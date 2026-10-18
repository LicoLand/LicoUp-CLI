//! Build-time compiler for declarative LicoUp state machines.
//!
//! Runtime crates keep their effects and guards locally. This crate owns only
//! validation and deterministic generation of the state/event vocabulary and
//! transition lookup table declared by JSON configuration.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Error(String);

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Deserialize)]
struct Document {
    machines: Vec<Machine>,
}

#[derive(Debug, Deserialize)]
struct Machine {
    id: String,
    #[serde(default)]
    rust_module: Option<String>,
    states: Vec<StateDefinition>,
    events: Vec<String>,
    initial: String,
    #[serde(default)]
    terminal: Vec<String>,
    transitions: Vec<Transition>,
}

#[derive(Debug, Deserialize)]
struct StateDefinition {
    id: String,
    #[serde(default)]
    rust_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Transition {
    from_state: String,
    event: String,
    to_state: String,
}

struct CompiledMachine<'a> {
    definition: &'a Machine,
    module: String,
    states: Vec<NamedValue<'a>>,
    events: Vec<NamedValue<'a>>,
    initial_index: usize,
    terminal_indices: BTreeSet<usize>,
    transitions: BTreeMap<(usize, usize), usize>,
}

struct NamedValue<'a> {
    wire_name: &'a str,
    rust_name: String,
}

/// Compile every `.json` document in `input_directory` into
/// `$OUT_DIR/state_machines.rs`.
pub fn generate_directory(input_directory: impl AsRef<Path>) -> Result<PathBuf, Error> {
    let output_directory = std::env::var_os("OUT_DIR")
        .ok_or_else(|| Error::new("OUT_DIR is not set for state-machine generation"))?;
    let output = PathBuf::from(output_directory).join("state_machines.rs");
    compile_directory(input_directory, &output)?;
    Ok(output)
}

/// Compile one JSON document into a Rust source file.
pub fn compile_file(input: impl AsRef<Path>, output: impl AsRef<Path>) -> Result<(), Error> {
    let input = input.as_ref();
    println!("cargo:rerun-if-changed={}", input.display());
    let bytes = fs::read(input)
        .map_err(|error| Error::new(format!("failed to read {}: {error}", input.display())))?;
    let document: Document = serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("invalid JSON in {}: {error}", input.display())))?;
    let generated = compile_documents(&[(input.to_path_buf(), document)])?;
    write_output(output.as_ref(), &generated)
}

fn compile_directory(input_directory: impl AsRef<Path>, output: &Path) -> Result<(), Error> {
    let input_directory = input_directory.as_ref();
    println!("cargo:rerun-if-changed={}", input_directory.display());
    let entries = fs::read_dir(input_directory).map_err(|error| {
        Error::new(format!(
            "failed to read state-machine directory {}: {error}",
            input_directory.display()
        ))
    })?;
    let mut paths = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| Error::new(format!("failed to read directory entry: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| path.extension().and_then(|value| value.to_str()) == Some("json"));
    paths.sort();

    let mut documents = Vec::with_capacity(paths.len());
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = fs::read(&path)
            .map_err(|error| Error::new(format!("failed to read {}: {error}", path.display())))?;
        let document = serde_json::from_slice(&bytes)
            .map_err(|error| Error::new(format!("invalid JSON in {}: {error}", path.display())))?;
        documents.push((path, document));
    }
    let generated = compile_documents(&documents)?;
    write_output(output, &generated)
}

fn write_output(output: &Path, generated: &str) -> Result<(), Error> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            Error::new(format!("failed to create {}: {error}", parent.display()))
        })?;
    }
    fs::write(output, generated)
        .map_err(|error| Error::new(format!("failed to write {}: {error}", output.display())))
}

fn compile_documents(documents: &[(PathBuf, Document)]) -> Result<String, Error> {
    if documents.is_empty() {
        return Err(Error::new("no state-machine JSON documents were found"));
    }
    let mut machine_ids = BTreeSet::new();
    let mut module_names = BTreeSet::new();
    let mut machines = Vec::new();
    for (path, document) in documents {
        if document.machines.is_empty() {
            return Err(Error::new(format!(
                "{} declares no state machines",
                path.display()
            )));
        }
        for machine in &document.machines {
            if !machine_ids.insert(machine.id.clone()) {
                return Err(Error::new(format!("duplicate machine id {:?}", machine.id)));
            }
            let compiled = validate_machine(machine, path)?;
            if !module_names.insert(compiled.module.clone()) {
                return Err(Error::new(format!(
                    "machine {:?} produces duplicate Rust module {:?}",
                    machine.id, compiled.module
                )));
            }
            machines.push(compiled);
        }
    }
    machines.sort_by(|left, right| left.definition.id.cmp(&right.definition.id));

    let mut output = String::from(
        "// @generated by licoup-state-machine-codegen. Do not edit.\n\
         // The JSON configuration is the transition authority.\n\n",
    );
    for machine in machines {
        render_machine(&mut output, &machine).expect("writing to a String cannot fail");
    }
    Ok(output)
}

fn validate_machine<'a>(machine: &'a Machine, path: &Path) -> Result<CompiledMachine<'a>, Error> {
    if machine.id.is_empty() || machine.id.trim() != machine.id {
        return Err(Error::new(format!(
            "{} has an empty or whitespace-padded machine id {:?}",
            path.display(),
            machine.id
        )));
    }
    let module = machine
        .rust_module
        .clone()
        .unwrap_or_else(|| to_snake_identifier(&machine.id));
    validate_identifier(&module, "module", &machine.id)?;

    if machine.states.is_empty() {
        return Err(Error::new(format!(
            "machine {:?} has no states",
            machine.id
        )));
    }
    if machine.events.is_empty() {
        return Err(Error::new(format!(
            "machine {:?} has no events",
            machine.id
        )));
    }

    let mut state_indexes = BTreeMap::new();
    let mut state_names = BTreeSet::new();
    let mut states = Vec::with_capacity(machine.states.len());
    for state in &machine.states {
        if state.id.is_empty()
            || state.id.trim() != state.id
            || state_indexes.contains_key(state.id.as_str())
        {
            return Err(Error::new(format!(
                "machine {:?} has duplicate, empty, or whitespace-padded state {:?}",
                machine.id, state.id
            )));
        }
        let rust_name = state
            .rust_name
            .clone()
            .unwrap_or_else(|| to_pascal_identifier(&state.id));
        validate_identifier(&rust_name, "state", &machine.id)?;
        if !state_names.insert(rust_name.clone()) {
            return Err(Error::new(format!(
                "machine {:?} states produce duplicate Rust name {:?}",
                machine.id, rust_name
            )));
        }
        let index = states.len();
        state_indexes.insert(state.id.as_str(), index);
        states.push(NamedValue {
            wire_name: &state.id,
            rust_name,
        });
    }

    let mut event_indexes = BTreeMap::new();
    let mut event_names = BTreeSet::new();
    let mut events = Vec::with_capacity(machine.events.len());
    for event in &machine.events {
        if event.is_empty() || event.trim() != event || event_indexes.contains_key(event.as_str()) {
            return Err(Error::new(format!(
                "machine {:?} has duplicate, empty, or whitespace-padded event {:?}",
                machine.id, event
            )));
        }
        let rust_name = to_pascal_identifier(event);
        validate_identifier(&rust_name, "event", &machine.id)?;
        if !event_names.insert(rust_name.clone()) {
            return Err(Error::new(format!(
                "machine {:?} events produce duplicate Rust name {:?}",
                machine.id, rust_name
            )));
        }
        let index = events.len();
        event_indexes.insert(event.as_str(), index);
        events.push(NamedValue {
            wire_name: event,
            rust_name,
        });
    }

    let initial_index = lookup(&state_indexes, &machine.initial, machine, "initial state")?;
    let mut terminal_indices = BTreeSet::new();
    for terminal in &machine.terminal {
        terminal_indices.insert(lookup(&state_indexes, terminal, machine, "terminal state")?);
    }

    let mut transitions = BTreeMap::new();
    for transition in &machine.transitions {
        let from = lookup(
            &state_indexes,
            &transition.from_state,
            machine,
            "transition source",
        )?;
        let event = lookup(
            &event_indexes,
            &transition.event,
            machine,
            "transition event",
        )?;
        let to = lookup(
            &state_indexes,
            &transition.to_state,
            machine,
            "transition target",
        )?;
        if terminal_indices.contains(&from) && from != to {
            return Err(Error::new(format!(
                "machine {:?} terminal state {:?} has an escaping transition",
                machine.id, transition.from_state
            )));
        }
        if transitions.insert((from, event), to).is_some() {
            return Err(Error::new(format!(
                "machine {:?} has ambiguous transition from {:?} on {:?}",
                machine.id, transition.from_state, transition.event
            )));
        }
    }

    Ok(CompiledMachine {
        definition: machine,
        module,
        states,
        events,
        initial_index,
        terminal_indices,
        transitions,
    })
}

fn lookup(
    indexes: &BTreeMap<&str, usize>,
    name: &str,
    machine: &Machine,
    role: &str,
) -> Result<usize, Error> {
    indexes.get(name).copied().ok_or_else(|| {
        Error::new(format!(
            "machine {:?} references unknown {role} {:?}",
            machine.id, name
        ))
    })
}

fn render_machine(output: &mut String, machine: &CompiledMachine<'_>) -> fmt::Result {
    writeln!(
        output,
        "#[allow(dead_code, reason = \"generated state-machine inspection API is shared across runtime consumers\")]"
    )?;
    writeln!(output, "pub mod {} {{", machine.module)?;
    writeln!(
        output,
        "    pub const MACHINE_ID: &str = {:?};",
        machine.definition.id
    )?;
    render_enum(output, "State", &machine.states)?;
    render_enum(output, "Event", &machine.events)?;
    writeln!(output, "    #[derive(Clone, Copy, Debug, Eq, PartialEq)]")?;
    writeln!(output, "    pub struct Transition {{")?;
    writeln!(output, "        pub from: State,")?;
    writeln!(output, "        pub event: Event,")?;
    writeln!(output, "        pub to: State,")?;
    writeln!(output, "    }}")?;
    writeln!(
        output,
        "    pub const ALL_STATES: [State; {}] = [",
        machine.states.len()
    )?;
    for state in &machine.states {
        writeln!(output, "        State::{},", state.rust_name)?;
    }
    writeln!(output, "    ];")?;
    writeln!(
        output,
        "    pub const ALL_EVENTS: [Event; {}] = [",
        machine.events.len()
    )?;
    for event in &machine.events {
        writeln!(output, "        Event::{},", event.rust_name)?;
    }
    writeln!(output, "    ];")?;
    writeln!(
        output,
        "    pub const INITIAL: State = State::{};",
        machine.states[machine.initial_index].rust_name
    )?;

    render_string_methods(output, "State", &machine.states)?;
    render_string_methods(output, "Event", &machine.events)?;

    writeln!(output, "    pub const TRANSITIONS: &[Transition] = &[")?;
    for ((from, event), to) in &machine.transitions {
        writeln!(
            output,
            "        Transition {{ from: State::{}, event: Event::{}, to: State::{} }},",
            machine.states[*from].rust_name,
            machine.events[*event].rust_name,
            machine.states[*to].rust_name
        )?;
    }
    writeln!(output, "    ];")?;
    writeln!(
        output,
        "    const TRANSITION_TABLE: [[Option<State>; {}]; {}] = [",
        machine.events.len(),
        machine.states.len()
    )?;
    for state_index in 0..machine.states.len() {
        write!(output, "        [")?;
        for event_index in 0..machine.events.len() {
            match machine.transitions.get(&(state_index, event_index)) {
                Some(target) => write!(
                    output,
                    "Some(State::{}),",
                    machine.states[*target].rust_name
                )?,
                None => write!(output, "None,")?,
            }
        }
        writeln!(output, "],")?;
    }
    writeln!(output, "    ];")?;
    writeln!(
        output,
        "    pub const fn transition(state: State, event: Event) -> Option<State> {{"
    )?;
    writeln!(
        output,
        "        TRANSITION_TABLE[state as usize][event as usize]"
    )?;
    writeln!(output, "    }}")?;
    writeln!(
        output,
        "    pub const fn permits(state: State, next: State) -> bool {{"
    )?;
    writeln!(
        output,
        "        let transitions = &TRANSITION_TABLE[state as usize];"
    )?;
    writeln!(output, "        let mut event_index = 0;")?;
    writeln!(output, "        while event_index < transitions.len() {{")?;
    writeln!(
        output,
        "            if let Some(target) = transitions[event_index]"
    )?;
    writeln!(
        output,
        "                && target as usize == next as usize"
    )?;
    writeln!(output, "            {{")?;
    writeln!(output, "                return true;")?;
    writeln!(output, "            }}")?;
    writeln!(output, "            event_index += 1;")?;
    writeln!(output, "        }}")?;
    writeln!(output, "        false")?;
    writeln!(output, "    }}")?;

    write!(
        output,
        "    const TERMINAL: [bool; {}] = [",
        machine.states.len()
    )?;
    for index in 0..machine.states.len() {
        write!(output, "{},", machine.terminal_indices.contains(&index))?;
    }
    writeln!(output, "];")?;
    writeln!(output, "    pub const fn terminal(state: State) -> bool {{")?;
    writeln!(output, "        TERMINAL[state as usize]")?;
    writeln!(output, "    }}")?;
    writeln!(output, "}}\n")
}

fn render_enum(output: &mut String, name: &str, values: &[NamedValue<'_>]) -> fmt::Result {
    writeln!(
        output,
        "    #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize)]"
    )?;
    writeln!(output, "    #[repr(usize)]")?;
    writeln!(output, "    pub enum {name} {{")?;
    for value in values {
        writeln!(output, "        #[serde(rename = {:?})]", value.wire_name)?;
        writeln!(output, "        {},", value.rust_name)?;
    }
    writeln!(output, "    }}")
}

fn render_string_methods(
    output: &mut String,
    name: &str,
    values: &[NamedValue<'_>],
) -> fmt::Result {
    writeln!(output, "    impl {name} {{")?;
    writeln!(
        output,
        "        pub const fn as_str(self) -> &'static str {{"
    )?;
    writeln!(output, "            match self {{")?;
    for value in values {
        writeln!(
            output,
            "                Self::{} => {:?},",
            value.rust_name, value.wire_name
        )?;
    }
    writeln!(output, "            }}")?;
    writeln!(output, "        }}")?;
    writeln!(
        output,
        "        pub fn from_name(value: &str) -> Option<Self> {{"
    )?;
    writeln!(output, "            match value {{")?;
    for value in values {
        writeln!(
            output,
            "                {:?} => Some(Self::{}),",
            value.wire_name, value.rust_name
        )?;
    }
    writeln!(output, "                _ => None,")?;
    writeln!(output, "            }}")?;
    writeln!(output, "        }}")?;
    writeln!(output, "    }}")
}

fn to_snake_identifier(value: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('_');
            }
            output.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    output
}

fn to_pascal_identifier(value: &str) -> String {
    let mut output = String::new();
    let mut capitalize = true;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if capitalize {
                output.push(character.to_ascii_uppercase());
            } else {
                output.push(character);
            }
            capitalize = false;
        } else {
            capitalize = true;
        }
    }
    output
}

fn validate_identifier(value: &str, role: &str, machine: &str) -> Result<(), Error> {
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric());
    if valid && !RUST_KEYWORDS.contains(&value) {
        return Ok(());
    }
    Err(Error::new(format!(
        "machine {machine:?} has invalid Rust {role} identifier {value:?}"
    )))
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn",
];

use super::*;

pub(super) fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
    );
}

pub(super) fn print_usage() {
    eprintln!(
        "Usage:\n  licoup help\n  licoup rpc stdio  # local native command bridge\n  licoup rpc conversation  # durable native host; bidirectional licoup.stdio.v1 NDJSON\n\nCommands (generated from native admission):"
    );
    for line in licoup_native::ffi::commands::cli_command_help() {
        eprintln!("{line}");
    }
    eprintln!(
        "\nUse `licoup commands` for exact option kinds, requirements, constraints, and RPC methods.\nUse --stdin-json true to read one private JSON object from stdin instead of argv.\nCanonical conversation and strategy execution use the durable native host."
    );
}

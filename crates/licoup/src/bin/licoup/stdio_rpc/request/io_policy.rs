pub(crate) fn rpc_command_writes_external_stdout(args: &[String]) -> bool {
    (args.first().map(String::as_str) == Some("rpc")
        && args.get(1).map(String::as_str) == Some("call"))
        || (args.first().map(String::as_str) == Some("conversations")
            && args.get(1).map(String::as_str) == Some("stream"))
        || (args.first().map(String::as_str) == Some("gateway")
            && args.get(1).map(String::as_str) == Some("client-token"))
}

pub(crate) fn rpc_command_reads_external_stdin(args: &[String]) -> bool {
    args.windows(2).any(|pair| pair == ["--stdin-json", "true"])
}

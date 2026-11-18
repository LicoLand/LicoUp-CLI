use super::*;

#[test]
fn stdio_rpc_discards_oversized_line_without_losing_next_request() {
    let mut reader = Cursor::new(b"12345\n{}\n".to_vec());

    assert!(matches!(
        read_stdio_rpc_line(&mut reader, 4).unwrap(),
        StdioRpcLine::TooLarge
    ));
    match read_stdio_rpc_line(&mut reader, 4).unwrap() {
        StdioRpcLine::Request(bytes) => assert_eq!(bytes, b"{}"),
        _ => panic!("expected the request after the oversized line"),
    }
    assert!(matches!(
        read_stdio_rpc_line(&mut reader, 4).unwrap(),
        StdioRpcLine::Eof
    ));
}

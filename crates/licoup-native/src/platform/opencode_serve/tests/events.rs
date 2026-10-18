//! The client's SSE ingress and byte record, driven by the package's watcher.
//!
//! These are host-owned claims: the client owns the engine that frames the
//! stream and the diagnostic record the frames are filed in, and this tree owns
//! the test that binds both to the adapter package's own classification
//! (`licoup_agent_opencode::driver::watch_session_events`).

use licoup_agent_opencode::driver::{ServeStreamFailure, watch_session_events};

use crate::platform::opencode_host;

#[test]
fn watcher_captures_complete_target_tool_frame_before_text_projection() {
    use crate::platform::raw_execution::{RawExecutionObserver, RawExecutionScope};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex, atomic::AtomicBool, mpsc};
    // The port is answered by this client's own engine, exactly as composition
    // installs it. Installation is first-wins per process, so a repeat in this
    // test binary is refused rather than fatal.
    let _ = licoup_agent_opencode::port::serve::install(opencode_host::serve_port());
    let target = ": trace\r\nid: opaque-frame\r\nevent: tool.updated\r\ndata: {\"type\":\"tool.updated\",\"properties\":{\"sessionID\":\"target\",\"arguments\":{\"path\":\"synthetic\"},\"result\":{\"unknown\":[1,2]}}}\r\n\r\n";
    let other = "data: {\"type\":\"tool.updated\",\"properties\":{\"sessionID\":\"other\",\"result\":\"excluded\"}}\n\n";
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut headers = Vec::new();
        let mut byte = [0u8; 1];
        while !headers.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
        }
        let body = format!("{other}{target}");
        write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    let observer = RawExecutionObserver::new(move |source, _, raw| {
        sink.lock()
            .unwrap()
            .push((source.to_owned(), raw.to_owned()));
        Ok(())
    });
    let (sender, receiver) = mpsc::sync_channel(4);
    let url = format!("http://{address}/event");
    let watcher = std::thread::spawn(move || {
        let _scope = RawExecutionScope::enter(Some(observer));
        watch_session_events(&url, "target", &AtomicBool::new(false), &sender)
    });
    assert_eq!(watcher.join().unwrap(), Err(ServeStreamFailure::Closed));
    server.join().unwrap();
    assert!(receiver.try_recv().is_err());
    assert_eq!(
        records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| record.0 == "opencode.sse")
            .map(|record| record.1.as_str())
            .collect::<Vec<_>>(),
        [target]
    );
}

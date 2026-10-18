use super::super::http::{self, HttpFailure};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

#[test]
fn http_policy_allows_loopback_plaintext_and_remote_tls_only() {
    assert!(http::validate_url("http://127.0.0.1:24173/session").is_ok());
    assert!(http::validate_url("http://localhost:4097/session").is_ok());
    assert!(http::validate_url("http://[::1]:4097/session").is_ok());
    assert!(http::validate_url("https://agent.example/session").is_ok());
    assert_eq!(
        http::validate_url("http://agent.example/session").unwrap_err(),
        HttpFailure::InvalidUrl
    );
    assert_eq!(
        http::validate_url("https://token@agent.example/session").unwrap_err(),
        HttpFailure::InvalidUrl
    );
}

fn read_http_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        if stream.read(&mut byte).map(|read| read == 0).unwrap_or(true) {
            return None;
        }
        head.push(byte[0]);
    }
    let head_text = String::from_utf8_lossy(&head);
    let mut request_line = None;
    let mut content_length = 0usize;
    for (index, line) in head_text.split("\r\n").enumerate() {
        if index == 0 {
            request_line = Some(line.to_owned());
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).ok()?;
    }
    request_line.map(|line| (line, body))
}

fn write_json_response(stream: &mut TcpStream, body: &str) {
    let payload = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: keep-alive\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(payload.as_bytes()).unwrap();
    stream.flush().unwrap();
}

#[test]
fn shared_control_client_reuses_one_connection_for_sequential_calls() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let accepts = Arc::new(AtomicUsize::new(0));
    let accepts_for_server = Arc::clone(&accepts);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        accepts_for_server.fetch_add(1, Ordering::SeqCst);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut served = 0;
        while served < 2 {
            let Some((request_line, _body)) = read_http_request(&mut stream) else {
                break;
            };
            if !request_line.starts_with("POST ") {
                break;
            }
            write_json_response(&mut stream, r#"{"ok":true}"#);
            served += 1;
        }
    });
    let base = format!("http://{address}");
    let first = http::post_json_observed(
        &format!("{base}/session"),
        &json!({"probe": 1}),
        Some(Duration::from_secs(5)),
        "synthetic.http",
    );
    let second = http::post_json_observed(
        &format!("{base}/session"),
        &json!({"probe": 2}),
        Some(Duration::from_secs(5)),
        "synthetic.http",
    );
    server.join().unwrap();
    assert_eq!(first.unwrap()["ok"], true);
    assert_eq!(second.unwrap()["ok"], true);
    assert_eq!(accepts.load(Ordering::SeqCst), 1);
}

#[test]
fn http_failure_preserves_the_non_success_status_class() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_http_request(&mut stream).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 422 Unprocessable Entity\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            )
            .unwrap();
    });
    let failure = http::post_json_observed(
        &format!("http://{address}/session"),
        &json!({}),
        Some(Duration::from_secs(2)),
        "synthetic.http",
    )
    .unwrap_err();
    server.join().unwrap();
    assert_eq!(failure, HttpFailure::Status(422));
}

#[test]
fn observed_http_keeps_encoded_request_and_unparsed_error_body() {
    use licoup_foundation::platform::raw_execution::{
        RawExecutionDirection, RawExecutionObserver, RawExecutionScope,
    };
    use std::sync::Mutex;
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    let observer = RawExecutionObserver::new(move |source, direction, raw| {
        sink.lock()
            .unwrap()
            .push((source.to_owned(), direction, raw.to_owned()));
        Ok(())
    });
    let _scope = RawExecutionScope::enter(Some(observer));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let response_body = concat!(
        r#"  {"unknownMetadata": [1, 2], "toolResult": "full\nvalue"}"#,
        "\r\n"
    );
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let (_, body) = read_http_request(&mut stream).unwrap();
        let response = format!(
            "HTTP/1.1 422 Unprocessable Entity\r\ncontent-length: {}\r\nx-extra: first\r\nx-extra: second\r\nconnection: close\r\n\r\n{response_body}",
            response_body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        body
    });
    let body = json!({"prompt": "synthetic\ninput", "settings": {"new": true}});
    assert_eq!(
        http::post_json_observed(
            &format!("http://{address}/session/target/message"),
            &body,
            Some(Duration::from_secs(5)),
            "synthetic.http"
        )
        .unwrap_err(),
        HttpFailure::Status(422)
    );
    let sent = String::from_utf8(server.join().unwrap()).unwrap();
    let records = records.lock().unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.0 == "synthetic.http")
            .cloned()
            .collect::<Vec<_>>(),
        [
            (
                "synthetic.http".to_owned(),
                RawExecutionDirection::Sent,
                sent
            ),
            (
                "synthetic.http".to_owned(),
                RawExecutionDirection::Received,
                response_body.to_owned()
            ),
        ]
    );
    let request: serde_json::Value = serde_json::from_str(&records[0].2).unwrap();
    assert_eq!(records[0].0, "synthetic.http.request-metadata");
    assert_eq!(request["method"], "POST");
    assert!(
        request["url"]
            .as_str()
            .unwrap()
            .ends_with("/session/target/message")
    );
    assert_eq!(
        request["headers"],
        json!([{"name":"content-type", "values":["application/json"]}])
    );
    let response: serde_json::Value = serde_json::from_str(&records[2].2).unwrap();
    assert_eq!(records[2].0, "synthetic.http.response-metadata");
    assert_eq!(response["status"], 422);
    assert!(
        response["headers"]
            .as_array()
            .unwrap()
            .contains(&json!({"name":"x-extra","values":["first","second"]}))
    );
}

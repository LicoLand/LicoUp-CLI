//! Bounded synthetic local HTTP fixtures.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};

pub(super) struct CapturedRequest {
    pub(super) method: String,
    pub(super) target: String,
    pub(super) headers: BTreeMap<String, String>,
    pub(super) body: Vec<u8>,
}

pub(super) fn serve_once(
    status: &'static str,
    content_type: &'static str,
    response_body: String,
) -> (String, JoinHandle<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fixture");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept fixture request");
        let request = read_request(&mut stream);
        write!(
            stream,
            "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            response_body.len()
        )
        .expect("write fixture headers");
        stream
            .write_all(response_body.as_bytes())
            .expect("write fixture body");
        request
    });
    (format!("http://{address}"), server)
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut reader = BufReader::new(stream.try_clone().expect("clone fixture stream"));
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .expect("read request line");
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().expect("request method").to_string();
    let target = request_parts.next().expect("request target").to_string();
    let mut headers = BTreeMap::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read request header");
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().expect("content length");
            }
            headers.insert(name, value);
        }
    }
    assert!(
        content_length <= 1_114_112,
        "fixture request exceeded bound"
    );
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).expect("read request body");
    CapturedRequest {
        method,
        target,
        headers,
        body,
    }
}

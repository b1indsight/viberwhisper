//! Bounded local chat-completions fixture shared by cleanup and desktop delivery tests.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::mpsc,
    time::Duration,
};

pub(crate) struct Request {
    pub headers: String,
    pub body: serde_json::Value,
}

pub(crate) fn stub(responses: Vec<String>) -> (reqwest::Url, mpsc::Receiver<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for content in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut headers = String::new();
            let mut length = None;
            loop {
                let mut line = String::new();
                assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                headers.push_str(&line);
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; length.unwrap()];
            reader.read_exact(&mut body).unwrap();
            let request = Request {
                headers,
                body: serde_json::from_slice(&body).unwrap(),
            };
            let body = serde_json::json!({"choices":[{"message":{"content":content}}]}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
            let _ = sender.send(request);
        }
    });
    (
        reqwest::Url::parse(&format!("http://{address}/v1/chat/completions")).unwrap(),
        receiver,
    )
}

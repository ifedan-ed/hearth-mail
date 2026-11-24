//! A stand-in DAV server for the tests: a tiny HTTP/1.1 server on a
//! loopback port that records every request and answers through a handler
//! the test writes, so a test can both check what the client sent and
//! answer as Nextcloud would.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// A request as the stand-in received it.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    /// The value of header `name` (lower case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// What the stand-in answers.
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
    /// Wait this long before answering (for the timeout test).
    pub delay: Duration,
}

impl Answer {
    /// A 207 multistatus with `body`.
    pub fn multistatus(body: &str) -> Self {
        Self::new(207, body).header("Content-Type", "application/xml; charset=utf-8")
    }

    /// Any status with a body.
    pub fn new(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_owned(),
            delay: Duration::ZERO,
        }
    }

    /// Adds a header.
    pub fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_owned()));
        self
    }
}

type Handler = dyn Fn(&Recorded) -> Answer + Send + Sync;

/// A running stand-in server.
pub struct StandIn {
    /// `http://127.0.0.1:<port>`.
    pub origin: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl StandIn {
    /// Starts a server answering through `handler`.
    pub async fn start(handler: impl Fn(&Recorded) -> Answer + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let recorded = requests.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let handler = handler.clone();
                let recorded = recorded.clone();
                tokio::spawn(async move { serve(stream, handler, recorded).await });
            }
        });
        Self { origin, requests }
    }

    /// `origin` + `path`.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }

    /// The requests received so far.
    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

/// Reads one request from `stream`, answers it, and closes the connection.
async fn serve(mut stream: TcpStream, handler: Arc<Handler>, recorded: Arc<Mutex<Vec<Recorded>>>) {
    let mut buffer = Vec::new();
    let head_end = loop {
        let mut chunk = [0u8; 4096];
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let method = request_line.next().unwrap_or_default().to_owned();
    let path = request_line.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < length {
        let mut chunk = [0u8; 4096];
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    let request = Recorded {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    let answer = handler(&request);
    recorded.lock().unwrap().push(request);
    tokio::time::sleep(answer.delay).await;
    let mut response = format!(
        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        answer.status,
        answer.body.len()
    );
    for (name, value) in &answer.headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    response.push_str(&answer.body);
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

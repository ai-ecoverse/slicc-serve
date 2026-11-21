use crate::bridge::Bridge;
use serve_core::http;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Duration;
use tungstenite::handshake::derive_accept_key;
use tungstenite::protocol::{Role, WebSocketConfig};
use tungstenite::{Message, WebSocket};

pub struct Endpoint {
    pub port: u16,
    pub secret: String,
}

impl Endpoint {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/{}/", self.port, self.secret)
    }

    fn ws(&self) -> String {
        format!(
            "ws://127.0.0.1:{}/{}/devtools/browser",
            self.port, self.secret
        )
    }
}

fn bind() -> std::io::Result<TcpListener> {
    for _ in 0..20 {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        if !(9222..=9229).contains(&port) {
            return Ok(listener);
        }
    }
    Err(std::io::Error::other("no free port outside 9222-9229"))
}

pub fn start(bridge: Arc<Bridge>, secret: String) -> std::io::Result<Endpoint> {
    let listener = bind()?;
    let port = listener.local_addr()?.port();
    let endpoint = Arc::new(Endpoint { port, secret });
    let sweeper = bridge.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        sweeper.expire();
    });
    let shared = endpoint.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let bridge = bridge.clone();
            let endpoint = shared.clone();
            std::thread::spawn(move || connection(stream, &bridge, &endpoint));
        }
    });
    Ok(Endpoint {
        port,
        secret: endpoint.secret.clone(),
    })
}

fn reply(stream: &mut TcpStream, status: u16, reason: &str, body: &str, kind: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

fn loopback_origin(origin: &str) -> bool {
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    rest.is_some_and(|r| {
        let host = r.split(['/', ':']).next().unwrap_or("");
        host == "127.0.0.1" || host == "localhost"
    })
}

fn connection(mut stream: TcpStream, bridge: &Arc<Bridge>, endpoint: &Endpoint) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut carry = Vec::new();
    let Ok(req) = http::read_request(&mut stream, &mut carry) else {
        return;
    };
    let host = req.header("host").unwrap_or("").to_ascii_lowercase();
    let port = endpoint.port;
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return reply(&mut stream, 403, "Forbidden", "wrong host\n", "text/plain");
    }
    let path = req.target.split(['?', '#']).next().unwrap_or("");
    let prefix = format!("/{}/", endpoint.secret);
    let Some(rest) = path.strip_prefix(&prefix) else {
        return reply(&mut stream, 404, "Not Found", "not found\n", "text/plain");
    };
    let rest = rest.trim_end_matches('/');
    if req.method != "GET" {
        return reply(
            &mut stream,
            405,
            "Method Not Allowed",
            "GET only\n",
            "text/plain",
        );
    }
    match rest {
        "json/version" => {
            let body = bridge.version_info(&endpoint.ws()).to_string();
            reply(&mut stream, 200, "OK", &body, "application/json")
        }
        "json" | "json/list" => {
            let body = bridge.list().to_string();
            reply(&mut stream, 200, "OK", &body, "application/json")
        }
        "devtools/browser" => {
            if let Some(origin) = req.header("origin") {
                if !loopback_origin(origin) {
                    return reply(
                        &mut stream,
                        403,
                        "Forbidden",
                        "no web pages\n",
                        "text/plain",
                    );
                }
            }
            let upgrade = req
                .header("upgrade")
                .is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
            let Some(key) = req.header("sec-websocket-key").filter(|_| upgrade) else {
                return reply(
                    &mut stream,
                    400,
                    "Bad Request",
                    "a WebSocket upgrade\n",
                    "text/plain",
                );
            };
            let accept = derive_accept_key(key.as_bytes());
            let _ = write!(
                stream,
                "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: {accept}\r\n\r\n"
            );
            browser(stream, carry, bridge);
        }
        _ => reply(&mut stream, 404, "Not Found", "not found\n", "text/plain"),
    }
}

struct Joined {
    head: std::io::Cursor<Vec<u8>>,
    stream: TcpStream,
}

impl Read for Joined {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if (self.head.position() as usize) < self.head.get_ref().len() {
            return self.head.read(buf);
        }
        self.stream.read(buf)
    }
}

impl Write for Joined {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

fn browser(stream: TcpStream, carry: Vec<u8>, bridge: &Arc<Bridge>) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(256 << 20);
    let joined = Joined {
        head: std::io::Cursor::new(carry),
        stream,
    };
    let mut socket = WebSocket::from_raw_socket(joined, Role::Server, Some(config));
    let (tx, rx) = channel();
    let client = bridge.open(tx);
    loop {
        let mut broken = false;
        while let Ok(text) = rx.try_recv() {
            if socket.send(Message::text(text)).is_err() {
                broken = true;
                break;
            }
        }
        if broken {
            break;
        }
        match socket.read() {
            Ok(Message::Text(text)) => bridge.message(client, text.as_str()),
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    bridge.close(client);
    let _ = socket.close(None);
    let _ = socket.flush();
}

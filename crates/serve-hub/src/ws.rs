use crate::hub::{classify, same_origin, Hub, HubError, Raw, Step, MAX_HOPS};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;
use tungstenite::client::IntoClientRequest;
use tungstenite::handshake::HandshakeError;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::WebSocket;
use url::Url;

pub type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

fn http_twin(url: &Url) -> Url {
    let mut twin = url.clone();
    let scheme = if url.scheme() == "wss" {
        "https"
    } else {
        "http"
    };
    let _ = twin.set_scheme(scheme);
    twin
}

fn ws_twin(url: &Url) -> Url {
    let mut twin = url.clone();
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    let _ = twin.set_scheme(scheme);
    twin
}

pub fn tcp(socket: &Socket) -> &TcpStream {
    match socket.get_ref() {
        MaybeTlsStream::Rustls(s) => s.get_ref(),
        MaybeTlsStream::Plain(s) => s,
        _ => unreachable!(),
    }
}

enum Opened {
    Socket(Box<Socket>),
    Answer(Raw),
    Failed(String),
}

fn open(url: &Url, bearer: Option<&str>) -> Opened {
    let Some(host) = url.host_str() else {
        return Opened::Failed("no host".into());
    };
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = match (host, port).to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(e) => return Opened::Failed(e.to_string()),
    };
    let mut last = String::from("no address");
    let mut stream = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(20)) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = e.to_string(),
        }
    }
    let Some(stream) = stream else {
        return Opened::Failed(last);
    };
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut request = match url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => return Opened::Failed(e.to_string()),
    };
    if let Ok(agent) = format!("{}/{}", serve_core::NAME, serve_core::version()).parse() {
        request.headers_mut().insert("user-agent", agent);
    }
    if let Some(value) = bearer.and_then(|b| format!("Bearer {b}").parse().ok()) {
        request.headers_mut().insert("authorization", value);
    }
    match tungstenite::client_tls_with_config(request, stream, None, None) {
        Ok((socket, _)) => Opened::Socket(Box::new(socket)),
        Err(HandshakeError::Failure(tungstenite::Error::Http(response))) => {
            let status = response.status().as_u16();
            let body = response.body().clone().unwrap_or_default();
            Opened::Answer(Raw::from_headers(status, response.headers(), body))
        }
        Err(HandshakeError::Failure(e)) => Opened::Failed(e.to_string()),
        Err(HandshakeError::Interrupted(_)) => Opened::Failed("the handshake stalled".into()),
    }
}

pub fn connect(hub: &Hub, url: &str, bearer: &str) -> Result<Socket, HubError> {
    let mut url = Url::parse(url).map_err(|e| HubError::Protocol {
        message: format!("the serve hub gave an invalid WebSocket URL: {e}"),
    })?;
    let mut hops = 0;
    let mut attempt = 0;
    loop {
        let bearer = same_origin(&http_twin(&url), &hub.base).then_some(bearer);
        let (error, retry) = match open(&url, bearer) {
            Opened::Socket(socket) => {
                let _ = tcp(&socket).set_read_timeout(Some(Duration::from_millis(25)));
                return Ok(*socket);
            }
            Opened::Answer(raw) => match classify(&hub.base, &http_twin(&url), &raw) {
                Step::Done => (
                    HubError::Protocol {
                        message: format!(
                            "the serve hub answered the WebSocket upgrade with {}",
                            raw.status
                        ),
                    },
                    false,
                ),
                Step::Follow(next) => {
                    hops += 1;
                    if hops > MAX_HOPS {
                        return Err(HubError::Redirects);
                    }
                    url = ws_twin(&next);
                    attempt = 0;
                    continue;
                }
                Step::Retry(error) => (error, true),
                Step::Fail(error) => (error, false),
            },
            Opened::Failed(message) => (HubError::Network { message }, true),
        };
        if !retry || attempt >= hub.backoff.len() {
            return Err(error);
        }
        std::thread::sleep(hub.backoff[attempt]);
        attempt += 1;
    }
}

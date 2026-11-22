use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::mpsc::Sender;
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tungstenite::Message;

pub const CHUNK: usize = 64 * 1024;
pub const WINDOW: u64 = 1024 * 1024;
pub const MAX_HEAD: usize = 64 * 1024;
pub const MAX_HEADERS: usize = 256;
const CONNECT: Duration = Duration::from_secs(10);
const READ: Duration = Duration::from_secs(120);
const TICK: Duration = Duration::from_secs(1);
const HOP: [&str; 10] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u32,
    pub preview_token: String,
    pub method: String,
    pub url: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: bool,
}

pub fn parse(v: &Value) -> Option<Request> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let headers = v
        .get("headers")
        .and_then(Value::as_array)?
        .iter()
        .map(|pair| match pair.as_array().map(Vec::as_slice) {
            Some([Value::String(n), Value::String(v)]) => Some((n.to_ascii_lowercase(), v.clone())),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Request {
        id: u32::try_from(v.get("id")?.as_u64()?).ok()?,
        preview_token: text("previewToken")?,
        method: text("method")?,
        url: text("url").unwrap_or_default(),
        path: text("path")?,
        headers,
        body: v.get("body").and_then(Value::as_bool).unwrap_or(false),
    })
}

pub fn frame(id: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 4);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

pub fn unframe(data: &[u8]) -> Option<(u32, &[u8])> {
    if data.len() < 5 {
        return None;
    }
    let id = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    Some((id, &data[4..]))
}

fn tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn token(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(tchar)
}

fn clean(value: &str) -> bool {
    !value.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0))
}

fn forwarded_host(url: &str) -> Option<(String, String)> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let host = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    Some((host, parsed.scheme().to_string()))
}

pub struct Head {
    pub bytes: Vec<u8>,
    pub chunked: bool,
}

pub fn request_head(req: &Request, port: u16) -> Result<Head, String> {
    if !token(&req.method) {
        return Err(format!("invalid method {:?}", req.method));
    }
    if !req.path.starts_with('/') || !req.path.bytes().all(|b| (0x21..0x7f).contains(&b)) {
        return Err(format!("invalid path {:?}", req.path));
    }
    let mut text = format!(
        "{} {} HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\n",
        req.method, req.path
    );
    let mut length = false;
    for (name, value) in &req.headers {
        let skip = HOP.contains(&name.as_str())
            || name == "x-forwarded-host"
            || name == "x-forwarded-proto";
        if skip || !token(name) || !clean(value) {
            continue;
        }
        length |= name == "content-length";
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some((host, scheme)) = forwarded_host(&req.url) {
        text.push_str(&format!(
            "x-forwarded-host: {host}\r\nx-forwarded-proto: {scheme}\r\n"
        ));
    }
    let chunked = req.body && !length;
    if chunked {
        text.push_str("transfer-encoding: chunked\r\n");
    }
    text.push_str("connection: close\r\n\r\n");
    Ok(Head {
        bytes: text.into_bytes(),
        chunked,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

fn read_line<R: BufRead>(r: &mut R, budget: &mut usize) -> Result<String, String> {
    let mut line = Vec::new();
    let read = r
        .take(*budget as u64 + 1)
        .read_until(b'\n', &mut line)
        .map_err(|e| e.to_string())?;
    if read == 0 {
        return Err("it closed the connection".into());
    }
    if read > *budget || !line.ends_with(b"\n") {
        return Err("its headers are too large".into());
    }
    *budget -= read;
    line.pop();
    if line.ends_with(b"\r") {
        line.pop();
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

pub fn read_response<R: BufRead>(r: &mut R) -> Result<Response, String> {
    let mut budget = MAX_HEAD;
    loop {
        let status_line = read_line(r, &mut budget)?;
        let mut parts = status_line.splitn(3, ' ');
        let version = parts.next().unwrap_or("");
        let status = parts
            .next()
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|_| version.starts_with("HTTP/1."))
            .ok_or_else(|| format!("it didn't answer HTTP ({status_line:?})"))?;
        let mut headers: Vec<(String, String)> = Vec::new();
        loop {
            let line = read_line(r, &mut budget)?;
            if line.is_empty() {
                break;
            }
            if line.starts_with([' ', '\t']) {
                if let Some(last) = headers.last_mut() {
                    last.1.push(' ');
                    last.1.push_str(line.trim());
                }
                continue;
            }
            let Some((name, value)) = line.split_once(':') else {
                return Err(format!("it sent a malformed header ({line:?})"));
            };
            if !token(name) {
                return Err(format!("it sent a malformed header ({line:?})"));
            }
            headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
            if headers.len() > MAX_HEADERS {
                return Err("it sent too many headers".into());
            }
        }
        match status {
            101 => return Err("it switched protocols, which previews don't relay".into()),
            100..=199 => continue,
            200..=599 => return Ok(Response { status, headers }),
            _ => return Err(format!("it answered status {status}")),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Framing {
    Empty,
    Length(u64),
    Chunked,
    Close,
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

pub fn framing(method: &str, response: &Response) -> Result<Framing, String> {
    if method == "HEAD" || matches!(response.status, 204 | 304) {
        return Ok(Framing::Empty);
    }
    if let Some(te) = header(&response.headers, "transfer-encoding") {
        let last = te.rsplit(',').next().unwrap_or("").trim();
        return Ok(if last.eq_ignore_ascii_case("chunked") {
            Framing::Chunked
        } else {
            Framing::Close
        });
    }
    match header(&response.headers, "content-length") {
        Some(len) => len
            .trim()
            .parse::<u64>()
            .map(Framing::Length)
            .map_err(|_| format!("it sent an invalid Content-Length ({len:?})")),
        None => Ok(Framing::Close),
    }
}

pub fn local_location(value: &str, port: u16) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    let local = matches!(url.host_str()?, "127.0.0.1" | "localhost" | "[::1]");
    if !local || url.scheme() != "http" || url.port_or_known_default() != Some(port) {
        return None;
    }
    let mut path = url.path().to_string();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    if let Some(f) = url.fragment() {
        path.push('#');
        path.push_str(f);
    }
    Some(path)
}

pub fn response_headers(response: &Response, framing: Framing, port: u16) -> Value {
    let pairs: Vec<Value> = response
        .headers
        .iter()
        .filter(|(n, _)| {
            !matches!(
                n.as_str(),
                "connection" | "keep-alive" | "transfer-encoding" | "proxy-connection"
            ) && (n != "content-length" || framing != Framing::Chunked)
        })
        .map(|(n, v)| {
            let v = match n.as_str() {
                "location" => local_location(v, port).unwrap_or_else(|| v.clone()),
                _ => v.clone(),
            };
            json!([n, v])
        })
        .collect();
    Value::Array(pairs)
}

pub struct Body<R> {
    reader: R,
    framing: Framing,
    left: u64,
    done: bool,
}

impl<R: BufRead> Body<R> {
    pub fn new(reader: R, framing: Framing) -> Body<R> {
        let left = match framing {
            Framing::Length(n) => n,
            _ => 0,
        };
        Body {
            reader,
            framing,
            left,
            done: false,
        }
    }

    fn chunk_size(&mut self) -> Result<u64, String> {
        let mut budget = 4096;
        let line = read_line(&mut self.reader, &mut budget)?;
        let size = line.split(';').next().unwrap_or("").trim();
        u64::from_str_radix(size, 16).map_err(|_| format!("it sent a bad chunk size ({line:?})"))
    }

    fn fill(&mut self, max: u64) -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; usize::try_from(max.min(CHUNK as u64)).unwrap_or(CHUNK)];
        let n = self.reader.read(&mut buf).map_err(|e| e.to_string())?;
        buf.truncate(n);
        Ok(buf)
    }

    pub fn piece(&mut self) -> Result<Option<Vec<u8>>, String> {
        if self.done {
            return Ok(None);
        }
        match self.framing {
            Framing::Empty => {
                self.done = true;
                Ok(None)
            }
            Framing::Close => {
                let buf = self.fill(CHUNK as u64)?;
                self.done = buf.is_empty();
                Ok((!buf.is_empty()).then_some(buf))
            }
            Framing::Length(_) => {
                if self.left == 0 {
                    self.done = true;
                    return Ok(None);
                }
                let buf = self.fill(self.left)?;
                if buf.is_empty() {
                    return Err(format!("its body ended {} bytes early", self.left));
                }
                self.left -= buf.len() as u64;
                Ok(Some(buf))
            }
            Framing::Chunked => {
                if self.left == 0 {
                    let size = self.chunk_size()?;
                    if size == 0 {
                        let mut budget = MAX_HEAD;
                        while !read_line(&mut self.reader, &mut budget)?.is_empty() {}
                        self.done = true;
                        return Ok(None);
                    }
                    self.left = size;
                }
                let buf = self.fill(self.left)?;
                if buf.is_empty() {
                    return Err("its chunked body ended early".into());
                }
                self.left -= buf.len() as u64;
                if self.left == 0 {
                    let mut budget = 2;
                    if !read_line(&mut self.reader, &mut budget)?.is_empty() {
                        return Err("its chunked body is malformed".into());
                    }
                }
                Ok(Some(buf))
            }
        }
    }
}

#[derive(Default)]
struct Flow {
    acked: u64,
    cancelled: bool,
    draining: bool,
    body: VecDeque<Vec<u8>>,
    ended: bool,
    origin: Option<TcpStream>,
}

#[derive(Default)]
pub struct Stream {
    flow: Mutex<Flow>,
    changed: Condvar,
}

impl Stream {
    pub fn new(body: bool) -> Stream {
        let stream = Stream::default();
        stream.flow.lock().unwrap().ended = !body;
        stream
    }

    pub fn push(&self, bytes: &[u8]) -> bool {
        let mut flow = self.flow.lock().unwrap();
        if flow.draining || flow.cancelled || flow.ended {
            return false;
        }
        flow.body.push_back(bytes.to_vec());
        self.changed.notify_all();
        true
    }

    pub fn end(&self) {
        self.flow.lock().unwrap().ended = true;
        self.changed.notify_all();
    }

    pub fn ack(&self, bytes: u64) {
        self.flow.lock().unwrap().acked += bytes;
        self.changed.notify_all();
    }

    pub fn cancel(&self) {
        let mut flow = self.flow.lock().unwrap();
        flow.cancelled = true;
        if let Some(origin) = flow.origin.take() {
            let _ = origin.shutdown(Shutdown::Both);
        }
        self.changed.notify_all();
    }

    pub fn cancelled(&self) -> bool {
        self.flow.lock().unwrap().cancelled
    }

    fn drain(&self) -> u64 {
        let mut flow = self.flow.lock().unwrap();
        flow.draining = true;
        flow.body.drain(..).map(|b| b.len() as u64).sum()
    }

    fn hold(&self, origin: &TcpStream) -> bool {
        let mut flow = self.flow.lock().unwrap();
        flow.origin = origin.try_clone().ok();
        !flow.cancelled
    }

    fn next_body(&self) -> Option<Vec<u8>> {
        let mut flow = self.flow.lock().unwrap();
        loop {
            if flow.cancelled {
                return None;
            }
            if let Some(bytes) = flow.body.pop_front() {
                return Some(bytes);
            }
            if flow.ended {
                return None;
            }
            flow = self.changed.wait_timeout(flow, TICK).unwrap().0;
        }
    }

    fn room(&self, sent: u64, more: u64) -> bool {
        let mut flow = self.flow.lock().unwrap();
        loop {
            if flow.cancelled {
                return false;
            }
            if sent + more <= flow.acked + WINDOW {
                return true;
            }
            flow = self.changed.wait_timeout(flow, TICK).unwrap().0;
        }
    }
}

pub struct Outcome {
    pub status: u16,
    pub bytes: u64,
}

fn send(out: &Sender<Message>, value: Value) {
    let _ = out.send(Message::text(value.to_string()));
}

fn drained(req: &Request, stream: &Stream, out: &Sender<Message>) {
    let rest = stream.drain();
    if rest > 0 {
        send(
            out,
            json!({ "type": "http.ack", "id": req.id, "bytes": rest }),
        );
    }
}

fn respond(
    req: &Request,
    out: &Sender<Message>,
    stream: &Stream,
    status: u16,
    text: &str,
) -> Outcome {
    drained(req, stream, out);
    let bodied = req.method != "HEAD";
    send(
        out,
        json!({
            "type": "http.response",
            "id": req.id,
            "status": status,
            "headers": [["content-type", "text/plain; charset=utf-8"], ["content-length", text.len().to_string()], ["cache-control", "no-store"]],
            "body": bodied,
        }),
    );
    if bodied {
        let _ = out.send(Message::binary(frame(req.id, text.as_bytes())));
        send(out, json!({ "type": "http.response.end", "id": req.id }));
    }
    Outcome {
        status,
        bytes: text.len() as u64,
    }
}

fn cancel(req: &Request, out: &Sender<Message>, reason: &str) {
    send(
        out,
        json!({ "type": "http.cancel", "id": req.id, "reason": reason }),
    );
}

fn write_body(
    req: &Request,
    origin: &mut TcpStream,
    chunked: bool,
    stream: &Stream,
    out: &Sender<Message>,
) -> bool {
    while let Some(bytes) = stream.next_body() {
        let written = if chunked {
            origin
                .write_all(format!("{:x}\r\n", bytes.len()).as_bytes())
                .and_then(|_| origin.write_all(&bytes))
                .and_then(|_| origin.write_all(b"\r\n"))
        } else {
            origin.write_all(&bytes)
        };
        send(
            out,
            json!({ "type": "http.ack", "id": req.id, "bytes": bytes.len() }),
        );
        if written.is_err() {
            return false;
        }
    }
    if stream.cancelled() {
        return false;
    }
    !chunked || origin.write_all(b"0\r\n\r\n").is_ok()
}

pub fn serve(req: &Request, port: Option<u16>, stream: &Stream, out: &Sender<Message>) -> Outcome {
    let Some(port) = port else {
        return respond(
            req,
            out,
            stream,
            404,
            "this preview isn't served here any more\n",
        );
    };
    let head = match request_head(req, port) {
        Ok(head) => head,
        Err(e) => return respond(req, out, stream, 400, &format!("{e}\n")),
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut origin = match TcpStream::connect_timeout(&addr, CONNECT) {
        Ok(s) => s,
        Err(e) => {
            return respond(
                req,
                out,
                stream,
                502,
                &format!("nothing answers on 127.0.0.1:{port}: {e}\n"),
            )
        }
    };
    let _ = origin.set_nodelay(true);
    let _ = origin.set_read_timeout(Some(READ));
    if !stream.hold(&origin) {
        return Outcome {
            status: 0,
            bytes: 0,
        };
    }
    if origin.write_all(&head.bytes).is_ok() && req.body {
        write_body(req, &mut origin, head.chunked, stream, out);
    }
    drained(req, stream, out);
    if stream.cancelled() {
        return Outcome {
            status: 0,
            bytes: 0,
        };
    }
    let mut reader = BufReader::with_capacity(CHUNK, origin);
    let response = match read_response(&mut reader) {
        Ok(r) => r,
        Err(_) if stream.cancelled() => {
            return Outcome {
                status: 0,
                bytes: 0,
            }
        }
        Err(e) => {
            return respond(
                req,
                out,
                stream,
                502,
                &format!("127.0.0.1:{port} gave no valid answer: {e}\n"),
            )
        }
    };
    let framing = match framing(&req.method, &response) {
        Ok(f) => f,
        Err(e) => {
            return respond(
                req,
                out,
                stream,
                502,
                &format!("127.0.0.1:{port} gave no valid answer: {e}\n"),
            )
        }
    };
    let bodied = framing != Framing::Empty;
    send(
        out,
        json!({
            "type": "http.response",
            "id": req.id,
            "status": response.status,
            "headers": response_headers(&response, framing, port),
            "body": bodied,
        }),
    );
    let mut sent = 0u64;
    if !bodied {
        return Outcome {
            status: response.status,
            bytes: 0,
        };
    }
    let mut body = Body::new(reader, framing);
    loop {
        match body.piece() {
            Ok(Some(bytes)) => {
                if !stream.room(sent, bytes.len() as u64) {
                    break;
                }
                let _ = out.send(Message::binary(frame(req.id, &bytes)));
                sent += bytes.len() as u64;
            }
            Ok(None) => {
                send(out, json!({ "type": "http.response.end", "id": req.id }));
                break;
            }
            Err(_) if stream.cancelled() => break,
            Err(e) => {
                cancel(
                    req,
                    out,
                    &format!("127.0.0.1:{port} failed mid-answer: {e}"),
                );
                break;
            }
        }
    }
    Outcome {
        status: response.status,
        bytes: sent,
    }
}

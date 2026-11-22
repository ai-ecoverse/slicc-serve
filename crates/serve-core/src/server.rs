use crate::http::{self, ReadError, Request};
use crate::range::{self, Range};
use crate::safe::{self, Lookup};
use crate::{mime, time};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use wasix_net::{Shutdown, TcpListener, TcpStream};

const DISCARD: u64 = 1024 * 1024;
const LINGER: u64 = 64 * 1024 * 1024;

pub enum Body {
    Empty,
    Bytes(Vec<u8>),
    File { path: PathBuf, start: u64, len: u64 },
}

pub struct Reply {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Body,
}

impl Reply {
    fn text(status: u16, text: &str) -> Reply {
        Reply {
            status,
            headers: vec![("content-type", "text/plain; charset=utf-8".into())],
            body: Body::Bytes(text.as_bytes().to_vec()),
        }
    }

    pub fn len(&self) -> u64 {
        match &self.body {
            Body::Empty => 0,
            Body::Bytes(b) => b.len() as u64,
            Body::File { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct Site {
    root: PathBuf,
}

impl Site {
    pub fn new(root: &Path) -> io::Result<Site> {
        let root = std::fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(io::Error::other("not a directory"));
        }
        Ok(Site { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn handle(&self, req: &Request) -> Reply {
        if req.method != "GET" && req.method != "HEAD" {
            let mut reply = Reply::text(405, "method not allowed\n");
            reply.headers.push(("allow", "GET, HEAD".into()));
            return reply;
        }
        let parts = match safe::segments(&req.target) {
            Lookup::BadRequest => return Reply::text(400, "bad request\n"),
            Lookup::Hidden => return not_found(),
            Lookup::Segments(parts) => parts,
        };
        let candidate = parts.iter().fold(self.root.clone(), |p, s| p.join(s));
        let Some((real, meta)) = safe::found(&self.root, &candidate) else {
            return not_found();
        };
        if meta.is_dir() {
            let path = req.target.split(['?', '#']).next().unwrap_or("/");
            if !path.ends_with('/') {
                let query = req.target.find('?').map(|i| &req.target[i..]).unwrap_or("");
                return Reply {
                    status: 301,
                    headers: vec![("location", format!("{path}/{query}"))],
                    body: Body::Empty,
                };
            }
            let index = safe::found(&self.root, &real.join("index.html"));
            if let Some((index, meta)) = index.filter(|(_, m)| m.is_file()) {
                return file(&index, &meta, req);
            }
            return listing(&real, &parts);
        }
        if !meta.is_file() {
            return not_found();
        }
        file(&real, &meta, req)
    }
}

fn not_found() -> Reply {
    Reply::text(404, "not found\n")
}

fn file(path: &Path, meta: &std::fs::Metadata, req: &Request) -> Reply {
    let size = meta.len();
    let mut headers = vec![
        (
            "content-type",
            mime::for_path(&path.to_string_lossy()).to_string(),
        ),
        ("accept-ranges", "bytes".into()),
        ("cache-control", "no-cache".into()),
    ];
    headers.extend(
        meta.modified()
            .ok()
            .and_then(time::unix_secs)
            .map(|m| ("last-modified", time::http_date(m))),
    );
    let wanted = if req.header("if-range").is_some() {
        None
    } else {
        req.header("range")
    };
    match range::parse(wanted, size) {
        Range::Whole => Reply {
            status: 200,
            headers,
            body: Body::File {
                path: path.to_path_buf(),
                start: 0,
                len: size,
            },
        },
        Range::Unsatisfiable => {
            headers.retain(|(k, _)| *k != "content-type");
            headers.push(("content-range", format!("bytes */{size}")));
            Reply {
                status: 416,
                headers,
                body: Body::Empty,
            }
        }
        Range::Bytes { start, end } => {
            headers.push(("content-range", format!("bytes {start}-{end}/{size}")));
            Reply {
                status: 206,
                headers,
                body: Body::File {
                    path: path.to_path_buf(),
                    start,
                    len: end - start + 1,
                },
            }
        }
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn listing(dir: &Path, parts: &[String]) -> Reply {
    let mut entries: Vec<(String, bool)> = std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(Result::ok)
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    if safe::hidden(&name) {
                        return None;
                    }
                    let is_dir = std::fs::metadata(e.path()).ok()?.is_dir();
                    Some((name, is_dir))
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    let title = format!("/{}", parts.join("/"));
    let mut html = format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<title>{0}</title>\n<h1>{0}</h1>\n<ul>\n",
        escape(&title)
    );
    if !parts.is_empty() {
        html.push_str("<li><a href=\"../\">../</a></li>\n");
    }
    for (name, is_dir) in entries {
        let slash = if is_dir { "/" } else { "" };
        html.push_str(&format!(
            "<li><a href=\"{}{slash}\">{}{slash}</a></li>\n",
            safe::percent_encode_segment(&name),
            escape(&name)
        ));
    }
    html.push_str("</ul>\n");
    Reply {
        status: 200,
        headers: vec![
            ("content-type", "text/html; charset=utf-8".into()),
            ("cache-control", "no-cache".into()),
        ],
        body: Body::Bytes(html.into_bytes()),
    }
}

pub fn write_reply<W: Write>(
    out: &mut W,
    reply: &Reply,
    head_only: bool,
    keep_alive: bool,
) -> io::Result<()> {
    let mut headers: Vec<(&str, String)> =
        reply.headers.iter().map(|(k, v)| (*k, v.clone())).collect();
    headers.push(("content-length", reply.len().to_string()));
    headers.push(("x-content-type-options", "nosniff".into()));
    headers.push((
        "connection",
        if keep_alive { "keep-alive" } else { "close" }.into(),
    ));
    http::write_head(out, reply.status, &headers)?;
    if !head_only {
        match &reply.body {
            Body::Empty => {}
            Body::Bytes(bytes) => out.write_all(bytes)?,
            Body::File { path, start, len } => {
                let mut f = File::open(path)?;
                f.seek(SeekFrom::Start(*start))?;
                let copied = io::copy(&mut f.take(*len), out)?;
                if copied != *len {
                    return Err(io::Error::other("file shrank while it was sent"));
                }
            }
        }
    }
    out.flush()
}

pub type Log = Arc<dyn Fn(&Request, &Reply) + Send + Sync>;

pub fn connection(mut stream: TcpStream, site: &Site, log: &Log, keep_alive: bool) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(30)));
    let mut carry = Vec::new();
    loop {
        let req = match http::read_request(&mut stream, &mut carry) {
            Ok(req) => req,
            Err(ReadError::Closed) | Err(ReadError::Io(_)) => return,
            Err(ReadError::TooLarge) => {
                let _ = write_reply(&mut stream, &Reply::text(431, "too large\n"), false, false);
                return;
            }
            Err(ReadError::Malformed) => {
                let _ = write_reply(
                    &mut stream,
                    &Reply::text(400, "bad request\n"),
                    false,
                    false,
                );
                return;
            }
        };
        let declared = body_length(&req);
        let mut keep = keep_alive && req.keep_alive();
        if declared > DISCARD {
            keep = false;
        } else if !discard(&mut stream, &mut carry, declared) {
            return;
        }
        let reply = site.handle(&req);
        let sent = write_reply(&mut stream, &reply, req.method == "HEAD", keep);
        log(&req, &reply);
        if sent.is_err() || !keep {
            if declared > DISCARD {
                linger(&mut stream);
            }
            return;
        }
    }
}

pub fn body_length(req: &Request) -> u64 {
    if req.header("transfer-encoding").is_some() {
        return u64::MAX;
    }
    match req.header("content-length") {
        Some(len) => len.trim().parse().unwrap_or(u64::MAX),
        None => 0,
    }
}

fn discard(stream: &mut TcpStream, carry: &mut Vec<u8>, len: u64) -> bool {
    let held = carry.len().min(usize::try_from(len).unwrap_or(usize::MAX));
    carry.drain(..held);
    let mut rest = len - held as u64;
    let mut buf = [0u8; 8192];
    while rest > 0 {
        let want = buf.len().min(usize::try_from(rest).unwrap_or(buf.len()));
        match stream.read(&mut buf[..want]) {
            Ok(0) | Err(_) => return false,
            Ok(n) => rest -= n as u64,
        }
    }
    true
}

fn linger(stream: &mut TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let mut buf = [0u8; 8192];
    let mut read = 0u64;
    while read < LINGER {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => read += n as u64,
        }
    }
}

pub fn serve(listener: TcpListener, site: Arc<Site>, stop: Arc<AtomicBool>, log: Log) {
    for stream in listener.incoming().flatten() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        spawn(stream, site.clone(), log.clone());
    }
}

#[cfg(not(target_os = "wasi"))]
fn spawn(stream: TcpStream, site: Arc<Site>, log: Log) {
    std::thread::spawn(move || connection(stream, &site, &log, true));
}

#[cfg(target_os = "wasi")]
fn spawn(stream: TcpStream, site: Arc<Site>, log: Log) {
    connection(stream, &site, &log, false);
}

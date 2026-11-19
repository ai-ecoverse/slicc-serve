use serde_json::{json, Map, Value};
use serve_core::range::{content_range, unsatisfied_size};
use serve_core::safe::within_served_root;
use serve_core::{mime, MAX_FILE_BYTES, MAX_RANGE_BYTES};
use std::io::Read;
use std::time::Duration;

pub const CHUNK: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub port: u16,
    pub served_root: String,
    pub entry_path: String,
    pub entry_is_root: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub req_id: String,
    pub served_root: String,
    pub vfs_path: String,
    pub as_text: bool,
    pub range: Option<String>,
}

pub fn parse(v: &Value) -> Option<Request> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Some(Request {
        req_id: text("reqId")?,
        served_root: text("servedRoot")?,
        vfs_path: text("vfsPath")?,
        as_text: v.get("asText").and_then(Value::as_bool).unwrap_or(false),
        range: text("range"),
    })
}

pub fn origin_path(origin: &Origin, vfs_path: &str) -> String {
    if origin.entry_is_root && vfs_path == origin.entry_path {
        return "/".into();
    }
    let root = origin.served_root.trim_end_matches('/');
    match vfs_path.strip_prefix(root) {
        Some("") | None => "/".into(),
        Some(rest) => rest.to_string(),
    }
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .accept_encoding(ureq::config::AutoHeaderValue::None)
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(Duration::from_secs(25)))
        .build()
        .into()
}

pub struct Answer {
    pub messages: Vec<Value>,
    pub status: u16,
    pub bytes: u64,
    pub path: String,
}

pub fn error(req_id: &str, status: u16, reason: Option<String>, size: Option<u64>) -> Value {
    let mut v =
        json!({ "type": "preview.response", "reqId": req_id, "ok": false, "status": status });
    if let Some(reason) = reason {
        v["reason"] = json!(reason);
    }
    if let Some(size) = size {
        v["size"] = json!(size);
    }
    v
}

fn failed(
    req: &Request,
    path: String,
    status: u16,
    reason: Option<String>,
    size: Option<u64>,
) -> Answer {
    Answer {
        messages: vec![error(&req.req_id, status, reason, size)],
        status,
        bytes: 0,
        path,
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn split_text(text: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while rest.len() > CHUNK {
        let mut at = CHUNK;
        while !rest.is_char_boundary(at) {
            at -= 1;
        }
        pieces.push(&rest[..at]);
        rest = &rest[at..];
    }
    pieces.push(rest);
    pieces
}

pub fn chunks(
    req_id: &str,
    mime: &str,
    bytes: &[u8],
    text: bool,
    meta: &Map<String, Value>,
) -> Vec<Value> {
    let utf8 = if text {
        std::str::from_utf8(bytes).ok()
    } else {
        None
    };
    let encoded;
    let (pieces, encoding): (Vec<&str>, &str) = match utf8 {
        Some(s) => (split_text(s), "utf-8"),
        None => {
            encoded = base64(bytes);
            let mut pieces: Vec<&str> = encoded
                .as_bytes()
                .chunks(CHUNK)
                .map(|c| std::str::from_utf8(c).unwrap_or(""))
                .collect();
            if pieces.is_empty() {
                pieces.push("");
            }
            (pieces, "base64")
        }
    };
    let total = pieces.len();
    pieces
        .into_iter()
        .enumerate()
        .map(|(i, content)| {
            let mut v = json!({
                "type": "preview.response",
                "reqId": req_id,
                "ok": true,
                "mime": mime,
                "chunkIndex": i,
                "totalChunks": total,
                "content": content,
                "encoding": encoding,
            });
            for (k, val) in meta {
                v[k] = val.clone();
            }
            v
        })
        .collect()
}

fn same_origin(port: u16, next: &url::Url) -> bool {
    matches!(next.host_str(), Some("127.0.0.1") | Some("localhost"))
        && next.scheme() == "http"
        && next.port_or_known_default() == Some(port)
}

fn fetch(
    agent: &ureq::Agent,
    port: u16,
    path: &str,
    range: Option<&str>,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let mut target = format!("http://127.0.0.1:{port}{path}");
    let mut hops = 0;
    loop {
        let mut request = agent.get(&target).header("accept-encoding", "identity");
        if let Some(range) = range {
            request = request.header("range", range);
        }
        let response = request.call().map_err(|e| e.to_string())?;
        let redirect = matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308);
        let next = header(&response, "location")
            .filter(|_| redirect && hops < 5)
            .and_then(|loc| url::Url::parse(&target).ok()?.join(&loc).ok())
            .filter(|n| same_origin(port, n));
        match next {
            Some(next) => {
                target = next.to_string();
                hops += 1;
            }
            None => return Ok(response),
        }
    }
}

fn header(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

pub fn answer(req: &Request, origin: Option<&Origin>, agent: &ureq::Agent) -> Answer {
    if !within_served_root(&req.vfs_path, &req.served_root) {
        return failed(req, req.vfs_path.clone(), 403, None, None);
    }
    let Some(origin) = origin.filter(|o| o.served_root == req.served_root) else {
        return failed(
            req,
            req.vfs_path.clone(),
            404,
            Some("this preview is no longer served".into()),
            None,
        );
    };
    let path = origin_path(origin, &req.vfs_path);
    let mut response = match fetch(agent, origin.port, &path, req.range.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            return failed(
                req,
                path,
                502,
                Some(format!("nothing answers on 127.0.0.1:{}: {e}", origin.port)),
                None,
            )
        }
    };
    let status = response.status().as_u16();
    let mime =
        header(&response, "content-type").unwrap_or_else(|| mime::for_path(&path).to_string());
    let declared = header(&response, "content-length").and_then(|v| v.parse::<u64>().ok());
    match status {
        200 => {
            if declared.is_some_and(|n| n > MAX_FILE_BYTES) {
                return failed(req, path, 413, Some(too_large()), declared);
            }
            let mut body = Vec::new();
            let read = response
                .body_mut()
                .as_reader()
                .take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut body);
            if let Err(e) = read {
                return failed(req, path, 502, Some(e.to_string()), None);
            }
            if body.len() as u64 > MAX_FILE_BYTES {
                return failed(req, path, 413, Some(too_large()), None);
            }
            let mut meta = Map::new();
            meta.insert("status".into(), json!(200));
            meta.insert("size".into(), json!(body.len()));
            Answer {
                messages: chunks(&req.req_id, &mime, &body, req.as_text, &meta),
                status: 200,
                bytes: body.len() as u64,
                path,
            }
        }
        206 => {
            let Some((start, _, total)) = header(&response, "content-range")
                .as_deref()
                .and_then(content_range)
            else {
                return failed(
                    req,
                    path,
                    502,
                    Some("a 206 without Content-Range".into()),
                    None,
                );
            };
            let mut body = Vec::new();
            let read = response
                .body_mut()
                .as_reader()
                .take(MAX_RANGE_BYTES)
                .read_to_end(&mut body);
            if let Err(e) = read {
                return failed(req, path, 502, Some(e.to_string()), None);
            }
            if body.is_empty() {
                return failed(req, path, 416, None, Some(total));
            }
            let end = start + body.len() as u64 - 1;
            let mut meta = Map::new();
            meta.insert("status".into(), json!(206));
            meta.insert("size".into(), json!(total));
            meta.insert("range".into(), json!({ "start": start, "end": end }));
            Answer {
                messages: chunks(&req.req_id, &mime, &body, false, &meta),
                status: 206,
                bytes: body.len() as u64,
                path,
            }
        }
        416 => {
            let size = header(&response, "content-range")
                .as_deref()
                .and_then(unsatisfied_size);
            failed(req, path, 416, None, size)
        }
        _ => {
            let mut body = Vec::new();
            let _ = response
                .body_mut()
                .as_reader()
                .take(4096)
                .read_to_end(&mut body);
            let text = String::from_utf8_lossy(&body).trim().to_string();
            let reason = if text.is_empty() {
                format!("the server answered {status}")
            } else {
                text
            };
            failed(req, path, status, Some(reason), None)
        }
    }
}

fn too_large() -> String {
    format!(
        "preview file exceeds {} MiB limit",
        MAX_FILE_BYTES / 1024 / 1024
    )
}

use serde_json::Value;
use serve_core::exit::Exit;
use serve_core::{version, NAME};
use std::time::Duration;
use url::Url;

pub const MAX_HOPS: usize = 5;
pub const BACKOFF: [u64; 3] = [1, 2, 5];
pub const PREVIEW_NOT_FOUND: &str = "PREVIEW_NOT_FOUND";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubError {
    SignIn {
        location: String,
    },
    Refused {
        status: u16,
        message: String,
        login: Option<String>,
    },
    Quota {
        status: u16,
        message: String,
        retry_after: Option<String>,
    },
    Gone {
        status: u16,
        message: String,
        code: Option<String>,
    },
    Client {
        status: u16,
        message: String,
        code: Option<String>,
    },
    Server {
        status: u16,
        message: String,
    },
    Network {
        message: String,
    },
    Redirects,
    Protocol {
        message: String,
    },
}

impl HubError {
    pub fn exit(&self) -> Exit {
        match self {
            HubError::SignIn { .. } | HubError::Refused { .. } => Exit::Auth,
            HubError::Quota { .. } => Exit::Quota,
            _ => Exit::Failure,
        }
    }

    pub fn recreates(&self) -> bool {
        match self {
            HubError::Refused { .. } => true,
            HubError::Gone { code, .. } => code.as_deref() != Some(PREVIEW_NOT_FOUND),
            _ => false,
        }
    }

    pub fn sign_in(&self) -> Option<&str> {
        match self {
            HubError::SignIn { location } => Some(location),
            HubError::Refused {
                login: Some(login), ..
            } => Some(login),
            _ => None,
        }
    }

    pub fn lines(&self, doing: &str) -> Vec<String> {
        let mut lines = Vec::new();
        match self {
            HubError::SignIn { location } => {
                lines.push(format!(
                    "{NAME}: the serve hub wants you to sign in: {location}"
                ));
            }
            HubError::Refused {
                status,
                message,
                login,
            } => {
                lines.push(format!(
                    "{NAME}: {doing}: the serve hub refused it ({status}: {message})"
                ));
                if let Some(login) = login {
                    lines.push(format!("{NAME}: sign in at {login}"));
                }
            }
            HubError::Quota {
                status,
                message,
                retry_after,
            } => {
                lines.push(format!(
                    "{NAME}: {doing}: the serve hub's limit is reached ({status}: {message})"
                ));
                if let Some(after) = retry_after {
                    let after = if after.bytes().all(|b| b.is_ascii_digit()) {
                        format!("{after} s")
                    } else {
                        after.clone()
                    };
                    lines.push(format!("{NAME}: retry after {after}"));
                }
            }
            HubError::Gone {
                status, message, ..
            }
            | HubError::Client {
                status, message, ..
            } => {
                lines.push(format!(
                    "{NAME}: {doing}: the serve hub answered {status}: {message}"
                ));
            }
            HubError::Server { status, message } => lines.push(format!(
                "{NAME}: {doing}: the serve hub answered {status} {} times: {message}",
                BACKOFF.len() + 1
            )),
            HubError::Network { message } => lines.push(format!(
                "{NAME}: {doing}: the serve hub is unreachable: {message}"
            )),
            HubError::Redirects => lines.push(format!(
                "{NAME}: {doing}: the serve hub redirected more than {MAX_HOPS} times"
            )),
            HubError::Protocol { message } => lines.push(format!("{NAME}: {doing}: {message}")),
        }
        lines
    }
}

#[derive(Clone, Debug, Default)]
pub struct Raw {
    pub status: u16,
    pub location: Option<String>,
    pub www_authenticate: Option<String>,
    pub link: Option<String>,
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
}

impl Raw {
    pub fn from_headers(status: u16, headers: &ureq::http::HeaderMap, body: Vec<u8>) -> Raw {
        let get = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        Raw {
            status,
            location: get("location"),
            www_authenticate: get("www-authenticate"),
            link: get("link"),
            retry_after: get("retry-after"),
            body,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Done,
    Follow(Url),
    Retry(HubError),
    Fail(HubError),
}

pub fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

pub fn api_path(path: &str) -> bool {
    path.starts_with("/api/")
}

pub fn classify(base: &Url, current: &Url, raw: &Raw) -> Step {
    let status = raw.status;
    match status {
        200..=299 => Step::Done,
        300..=399 => {
            let Some(location) = raw.location.as_deref() else {
                return Step::Fail(HubError::Protocol {
                    message: format!("the serve hub answered {status} without a Location"),
                });
            };
            let Ok(target) = current.join(location) else {
                return Step::Fail(HubError::Protocol {
                    message: format!("the serve hub redirected to an invalid URL: {location}"),
                });
            };
            if same_origin(&target, base) && api_path(target.path()) {
                Step::Follow(target)
            } else {
                Step::Fail(HubError::SignIn {
                    location: target.to_string(),
                })
            }
        }
        401 | 403 => Step::Fail(HubError::Refused {
            status,
            message: message(raw),
            login: login(raw),
        }),
        402 | 429 => Step::Fail(HubError::Quota {
            status,
            message: message(raw),
            retry_after: raw.retry_after.clone(),
        }),
        404 | 410 => Step::Fail(HubError::Gone {
            status,
            message: message(raw),
            code: code(raw),
        }),
        400..=499 => Step::Fail(HubError::Client {
            status,
            message: message(raw),
            code: code(raw),
        }),
        500..=599 => Step::Retry(HubError::Server {
            status,
            message: message(raw),
        }),
        _ => Step::Fail(HubError::Protocol {
            message: format!("the serve hub answered {status}"),
        }),
    }
}

fn code(raw: &Raw) -> Option<String> {
    let json: Value = serde_json::from_slice(&raw.body).ok()?;
    json.get("code")?.as_str().map(str::to_string)
}

pub fn message(raw: &Raw) -> String {
    if let Ok(json) = serde_json::from_slice::<Value>(&raw.body) {
        let text = |k: &str| json.get(k).and_then(Value::as_str).map(str::to_string);
        let error = text("error").or_else(|| text("message"));
        let code = text("code");
        match (error, code) {
            (Some(e), Some(c)) => return format!("{e} ({c})"),
            (Some(e), None) => return e,
            (None, Some(c)) => return c,
            _ => {}
        }
    }
    let text = String::from_utf8_lossy(&raw.body);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    match line {
        Some(l) if l.chars().count() > 200 => {
            format!("{}…", l.chars().take(200).collect::<String>())
        }
        Some(l) => l.to_string(),
        None => format!("HTTP {}", raw.status),
    }
}

pub fn login(raw: &Raw) -> Option<String> {
    if let Some(link) = raw.link.as_deref() {
        for part in link.split(',') {
            let Some((target, params)) = part.split_once(';') else {
                continue;
            };
            let rel = params.split(';').any(|p| {
                let p = p.trim().to_ascii_lowercase();
                p == "rel=login" || p == "rel=\"login\""
            });
            let target = target.trim();
            if rel && target.starts_with('<') && target.ends_with('>') {
                return Some(target[1..target.len() - 1].to_string());
            }
        }
    }
    let auth = raw.www_authenticate.as_deref()?;
    for piece in auth.split('"') {
        if piece.starts_with("https://") || piece.starts_with("http://") {
            return Some(piece.to_string());
        }
    }
    Some(auth.to_string())
}

pub enum Payload {
    None,
    Json(Value),
    Bytes(Vec<u8>, String),
}

pub struct Hub {
    pub base: Url,
    agent: ureq::Agent,
    pub backoff: Vec<Duration>,
}

pub fn origin_of(url: &Url) -> String {
    url.origin().ascii_serialization()
}

impl Hub {
    pub fn new(base: &str) -> Result<Hub, String> {
        let mut base =
            Url::parse(base).map_err(|e| format!("invalid serve hub URL {base}: {e}"))?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(format!("invalid serve hub URL {base}"));
        }
        base.set_path("/");
        base.set_query(None);
        base.set_fragment(None);
        let agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(120)))
            .timeout_recv_body(Some(Duration::from_secs(120)))
            .user_agent(format!("{NAME}/{}", version()))
            .build()
            .into();
        Ok(Hub {
            base,
            agent,
            backoff: BACKOFF.iter().map(|s| Duration::from_secs(*s)).collect(),
        })
    }

    pub fn origin(&self) -> String {
        origin_of(&self.base)
    }

    pub fn url(&self, path: &str) -> Url {
        self.base.join(path).unwrap_or_else(|_| self.base.clone())
    }

    pub fn call(
        &self,
        method: &str,
        url: Url,
        bearer: Option<&str>,
        payload: Payload,
    ) -> Result<Value, HubError> {
        let mut url = url;
        let mut method = method.to_string();
        let mut payload = payload;
        let mut hops = 0;
        let mut attempt = 0;
        loop {
            let bearer = bearer.filter(|_| same_origin(&url, &self.base));
            let (error, retry) = match self.send(&method, &url, bearer, &payload) {
                Ok(raw) => match classify(&self.base, &url, &raw) {
                    Step::Done => {
                        return Ok(serde_json::from_slice(&raw.body).unwrap_or(Value::Null))
                    }
                    Step::Follow(next) => {
                        hops += 1;
                        if hops > MAX_HOPS {
                            return Err(HubError::Redirects);
                        }
                        let rewrites = raw.status == 303
                            || (matches!(raw.status, 301 | 302) && method == "POST");
                        if rewrites && method != "HEAD" {
                            method = "GET".into();
                            payload = Payload::None;
                        }
                        url = next;
                        attempt = 0;
                        continue;
                    }
                    Step::Retry(error) => (error, true),
                    Step::Fail(error) => (error, false),
                },
                Err(message) => (HubError::Network { message }, true),
            };
            if !retry || attempt >= self.backoff.len() {
                return Err(error);
            }
            std::thread::sleep(self.backoff[attempt]);
            attempt += 1;
        }
    }

    fn send(
        &self,
        method: &str,
        url: &Url,
        bearer: Option<&str>,
        payload: &Payload,
    ) -> Result<Raw, String> {
        let mut builder = ureq::http::Request::builder()
            .method(method)
            .uri(url.as_str());
        if let Some(token) = bearer {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let body = match payload {
            Payload::None => Vec::new(),
            Payload::Json(value) => {
                builder = builder.header("content-type", "application/json");
                serde_json::to_vec(value).unwrap_or_default()
            }
            Payload::Bytes(bytes, mime) => {
                builder = builder.header("content-type", mime.as_str());
                bytes.clone()
            }
        };
        let request = builder.body(body).map_err(|e| e.to_string())?;
        let mut response = self.agent.run(request).map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(4 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| e.to_string())?;
        Ok(Raw::from_headers(status, response.headers(), body))
    }
}

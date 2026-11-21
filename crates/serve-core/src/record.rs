use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tray_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_root: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default)]
    pub foreground: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bridge: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cdp_url: Option<String>,
}

impl Record {
    pub fn shown_url(&self) -> &str {
        self.url
            .as_deref()
            .or(self.local_url.as_deref())
            .unwrap_or("-")
    }

    pub fn matches(&self, target: &str) -> bool {
        if self.id == target || self.preview_token.as_deref() == Some(target) {
            return true;
        }
        if let Some(port) = loopback_port(target) {
            return self.port == Some(port);
        }
        let trimmed = target.trim_end_matches('/');
        [self.url.as_deref(), self.local_url.as_deref()]
            .into_iter()
            .flatten()
            .any(|u| u.trim_end_matches('/') == trimmed || origin(u) == origin(target))
    }
}

pub enum Found {
    None,
    One(Box<Record>),
    Many(usize),
}

pub fn find(records: Vec<Record>, target: &str) -> Found {
    let mut hits: Vec<Record> = records.into_iter().filter(|r| r.matches(target)).collect();
    match hits.len() {
        0 => Found::None,
        1 => Found::One(Box::new(hits.remove(0))),
        n => Found::Many(n),
    }
}

fn port_number(text: &str) -> Option<u16> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|p| *p > 0)
}

pub fn loopback_port(target: &str) -> Option<u16> {
    if let Some(port) = port_number(target) {
        return Some(port);
    }
    let rest = target.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let host = host.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    let loopback = host == "localhost"
        || host.ends_with(".localhost")
        || host.strip_prefix("127.").is_some_and(|r| {
            r.split('.').count() == 3 && r.split('.').all(|o| o.parse::<u8>().is_ok())
        });
    if !loopback {
        return None;
    }
    match port {
        Some(port) => port_number(port),
        None => host
            .split('.')
            .next()
            .and_then(port_number)
            .filter(|_| host.ends_with(".localhost")),
    }
}

fn origin(url: &str) -> Option<&str> {
    let start = url.find("://")? + 3;
    let end = url[start..]
        .find(['/', '?', '#'])
        .map(|i| start + i)
        .unwrap_or(url.len());
    Some(&url[..end])
}

pub fn public_url(url: &str, explicit_entry: bool) -> String {
    if explicit_entry {
        return url.to_string();
    }
    match url.strip_suffix("index.html") {
        Some(base) if base.ends_with('/') => base.to_string(),
        _ => url.to_string(),
    }
}

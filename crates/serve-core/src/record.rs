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
}

impl Record {
    pub fn shown_url(&self) -> &str {
        self.url
            .as_deref()
            .or(self.local_url.as_deref())
            .unwrap_or("-")
    }

    pub fn matches(&self, target: &str) -> bool {
        let trimmed = target.trim_end_matches('/');
        self.id == target
            || self.preview_token.as_deref() == Some(target)
            || [self.url.as_deref(), self.local_url.as_deref()]
                .into_iter()
                .flatten()
                .any(|u| u.trim_end_matches('/') == trimmed || origin(u) == origin(target))
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

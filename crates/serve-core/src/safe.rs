use std::path::{Path, PathBuf};

pub fn within_served_root(vfs_path: &str, served_root: &str) -> bool {
    if vfs_path.is_empty() || served_root.is_empty() || served_root == "/" {
        return false;
    }
    if !vfs_path.starts_with('/') || vfs_path.to_ascii_lowercase().contains("%2e") {
        return false;
    }
    if vfs_path.split('/').any(|s| s == "." || s == "..") {
        return false;
    }
    let root = served_root.strip_suffix('/').unwrap_or(served_root);
    vfs_path == root || vfs_path.starts_with(&format!("{root}/"))
}

pub fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn percent_encode_segment(segment: &str) -> String {
    let mut out = String::new();
    for b in segment.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    BadRequest,
    Hidden,
    Segments(Vec<String>),
}

pub fn segments(target: &str) -> Lookup {
    let path = target.split(['?', '#']).next().unwrap_or("");
    if !path.starts_with('/') {
        return Lookup::BadRequest;
    }
    let Some(decoded) = percent_decode(path) else {
        return Lookup::BadRequest;
    };
    let mut parts = Vec::new();
    for part in decoded.split('/') {
        if part.is_empty() {
            continue;
        }
        if part == "." || part == ".." || part.contains(['\\', '\0', ':']) {
            return Lookup::BadRequest;
        }
        if part.starts_with('.') && part != ".well-known" {
            return Lookup::Hidden;
        }
        parts.push(part.to_string());
    }
    Lookup::Segments(parts)
}

pub fn hidden(name: &str) -> bool {
    name.starts_with('.') && name != ".well-known"
}

pub fn inside(root: &Path, candidate: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(candidate).ok()?;
    real.starts_with(root).then_some(real)
}

pub fn found(root: &Path, candidate: &Path) -> Option<(PathBuf, std::fs::Metadata)> {
    let real = inside(root, candidate)?;
    let meta = std::fs::metadata(&real).ok()?;
    Some((real, meta))
}

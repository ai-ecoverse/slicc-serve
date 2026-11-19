use crate::{mime, safe, MAX_FILE_BYTES, MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_FILES};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub abs: PathBuf,
    pub size: u64,
    pub mime: &'static str,
}

pub fn collect(root: &Path) -> Result<Vec<File>, String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    let mut total = 0u64;
    walk(&root, &root, &mut Vec::new(), &mut files, &mut total)?;
    Ok(files)
}

fn walk(
    root: &Path,
    dir: &Path,
    prefix: &mut Vec<String>,
    files: &mut Vec<File>,
    total: &mut u64,
) -> Result<(), String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| !safe::hidden(name))
        .collect();
    names.sort();
    for name in names {
        let Some((real, meta)) = safe::found(root, &dir.join(&name)) else {
            continue;
        };
        prefix.push(name.clone());
        let rel = prefix.join("/");
        if meta.is_dir() {
            if !dir.starts_with(&real) {
                walk(root, &real, prefix, files, total)?;
            }
        } else if meta.is_file() {
            if name.contains(['\\', ':']) {
                return Err(format!("can't snapshot {rel}: its name has a \\ or :"));
            }
            if files.len() >= MAX_SNAPSHOT_FILES {
                return Err(format!(
                    "the snapshot exceeds {MAX_SNAPSHOT_FILES} files (hidden files excluded)"
                ));
            }
            if meta.len() > MAX_FILE_BYTES {
                return Err(format!("{rel} exceeds the 25 MiB limit per file"));
            }
            *total += meta.len();
            if *total > MAX_SNAPSHOT_BYTES {
                return Err("the snapshot exceeds 50 MiB in total".into());
            }
            files.push(File {
                mime: mime::for_path(&rel),
                path: rel,
                abs: real,
                size: meta.len(),
            });
        }
        prefix.pop();
    }
    Ok(())
}

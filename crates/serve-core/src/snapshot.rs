use crate::{
    mime, safe, MAX_FILE_BYTES, MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_DIRS, MAX_SNAPSHOT_FILES,
};
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
    let mut walk = Walk {
        root: root.clone(),
        stack: vec![root.clone()],
        dirs: 0,
        files: Vec::new(),
        total: 0,
    };
    walk.dir(&root, &mut Vec::new())?;
    Ok(walk.files)
}

struct Walk {
    root: PathBuf,
    stack: Vec<PathBuf>,
    dirs: usize,
    files: Vec<File>,
    total: u64,
}

impl Walk {
    fn dir(&mut self, dir: &Path, prefix: &mut Vec<String>) -> Result<(), String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|name| !safe::hidden(name))
            .collect();
        names.sort();
        for name in names {
            let Some((real, meta)) = safe::found(&self.root, &dir.join(&name)) else {
                continue;
            };
            prefix.push(name.clone());
            let rel = prefix.join("/");
            if meta.is_dir() {
                if !self.stack.contains(&real) {
                    self.dirs += 1;
                    if self.dirs > MAX_SNAPSHOT_DIRS {
                        return Err(format!(
                            "the snapshot walks more than {MAX_SNAPSHOT_DIRS} directories (count each symlinked directory once per path to it)"
                        ));
                    }
                    self.stack.push(real.clone());
                    let walked = self.dir(&real, prefix);
                    self.stack.pop();
                    walked?;
                }
            } else if meta.is_file() {
                if name.contains(['\\', ':']) {
                    return Err(format!("can't snapshot {rel}: its name has a \\ or :"));
                }
                if self.files.len() >= MAX_SNAPSHOT_FILES {
                    return Err(format!(
                        "the snapshot exceeds {MAX_SNAPSHOT_FILES} files (hidden files excluded)"
                    ));
                }
                if meta.len() > MAX_FILE_BYTES {
                    return Err(format!("{rel} exceeds the 25 MiB limit per file"));
                }
                self.total += meta.len();
                if self.total > MAX_SNAPSHOT_BYTES {
                    return Err("the snapshot exceeds 50 MiB in total".into());
                }
                self.files.push(File {
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
}

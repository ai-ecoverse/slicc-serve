use crate::fsx;
use serve_core::record::{self, Found, Record};
use std::path::PathBuf;

#[derive(Clone)]
pub struct Records {
    dir: PathBuf,
}

impl Records {
    pub fn new(state: &std::path::Path) -> Records {
        Records {
            dir: state.join("previews"),
        }
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn save(&self, record: &Record) -> std::io::Result<()> {
        fsx::write_json(&self.path(&record.id), record)
    }

    pub fn remove(&self, id: &str) {
        let _ = std::fs::remove_file(self.path(id));
    }

    pub fn all(&self) -> Vec<Record> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut records: Vec<Record> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
            .filter_map(|e| fsx::read_json(&e.path()))
            .collect();
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        records
    }

    pub fn find(&self, target: &str) -> Found {
        record::find(self.all(), target)
    }

    pub fn holds_snapshots(&self, tray_id: &str) -> bool {
        let now = serve_core::time::iso(serve_core::time::now_ms());
        self.all().iter().any(|r| {
            r.mode == "persistent"
                && r.tray_id.as_deref() == Some(tray_id)
                && r.expires_at.as_deref().is_none_or(|e| e > now.as_str())
        })
    }
}

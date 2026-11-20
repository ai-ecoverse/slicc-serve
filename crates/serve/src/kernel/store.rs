use super::sys;
use serve_core::record::Record;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct Store {
    pub state: PathBuf,
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf));
    if read.is_err() {
        let seed = serve_core::time::now_ms() ^ (u64::from(sys::pid()) << 32);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (seed >> ((i % 8) * 8)) as u8 ^ (i as u8).wrapping_mul(151);
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

impl Store {
    pub fn new() -> Store {
        Store {
            state: serve_core::paths::from_process().state,
        }
    }

    fn previews(&self) -> PathBuf {
        self.state.join("previews")
    }

    pub fn log(&self) -> PathBuf {
        self.state.join("serve.log")
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.previews())?;
        sys::chmod(&self.state, 0o700);
        sys::chmod(&self.previews(), 0o700);
        Ok(())
    }

    fn path(&self, id: &str) -> PathBuf {
        self.previews().join(format!("{id}.json"))
    }

    pub fn save(&self, record: &Record) -> std::io::Result<()> {
        self.ensure()?;
        let target = self.path(&record.id);
        let temp = self
            .previews()
            .join(format!(".{}.{}.tmp", record.id, random_hex(4)));
        let written = write_private(&temp, record);
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
            return written;
        }
        std::fs::rename(&temp, &target)
    }

    pub fn remove(&self, id: &str) {
        let _ = std::fs::remove_file(self.path(id));
    }

    pub fn all(&self) -> Vec<Record> {
        let Ok(entries) = std::fs::read_dir(self.previews()) else {
            return Vec::new();
        };
        let mut records: Vec<Record> = entries
            .filter_map(Result::ok)
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.ends_with(".json") && !name.starts_with('.')
            })
            .filter_map(|e| std::fs::read(e.path()).ok())
            .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
            .collect();
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        records
    }

    pub fn living(&self) -> Vec<Record> {
        self.all()
            .into_iter()
            .filter(|r| {
                let alive = r.pid.is_some_and(sys::alive);
                if !alive {
                    self.remove(&r.id);
                }
                alive
            })
            .collect()
    }
}

fn write_private(path: &Path, record: &Record) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    sys::chmod(path, 0o600);
    let mut text = serde_json::to_vec_pretty(record).map_err(std::io::Error::other)?;
    text.push(b'\n');
    file.write_all(&text)?;
    file.flush()
}

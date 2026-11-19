use std::path::PathBuf;

pub struct Dirs {
    pub config: PathBuf,
    pub state: PathBuf,
}

pub fn dirs(var: &dyn Fn(&str) -> Option<String>, windows: bool) -> Dirs {
    let get = |name: &str| var(name).filter(|v| !v.is_empty());
    let home = || {
        PathBuf::from(
            get(if windows { "USERPROFILE" } else { "HOME" }).unwrap_or_else(|| ".".into()),
        )
    };
    let config = get("SCSERVE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if windows {
                get("APPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home().join("AppData").join("Roaming"))
            } else {
                get("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home().join(".config"))
            }
            .join("slicc")
            .join("serve")
        });
    let state = get("SCSERVE_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if windows {
                get("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home().join("AppData").join("Local"))
            } else {
                get("XDG_STATE_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home().join(".local").join("state"))
            }
            .join("slicc")
            .join("serve")
        });
    Dirs { config, state }
}

pub fn from_process() -> Dirs {
    dirs(&|name| std::env::var(name).ok(), cfg!(windows))
}

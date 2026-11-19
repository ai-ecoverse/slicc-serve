use std::path::PathBuf;
use std::process::{Command, Stdio};

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .map(|e| e.to_ascii_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn opener() -> Option<(PathBuf, Vec<&'static str>)> {
    let tries: &[(&str, &[&'static str])] = if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else if cfg!(windows) {
        &[("explorer", &[])]
    } else {
        &[("xdg-open", &[]), ("gio", &["open"])]
    };
    tries
        .iter()
        .find_map(|(name, args)| on_path(name).map(|p| (p, args.to_vec())))
}

pub fn open(url: &str) -> Result<(), String> {
    let Some((program, args)) = opener() else {
        let tried = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(windows) {
            "explorer"
        } else {
            "xdg-open, gio"
        };
        return Err(format!("no opener found (tried {tried})"));
    };
    let status = Command::new(&program)
        .args(args)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    if status.success() || cfg!(windows) {
        Ok(())
    } else {
        Err(format!("{} exited with {status}", program.display()))
    }
}

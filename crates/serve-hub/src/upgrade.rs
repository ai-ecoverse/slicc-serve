use serve_core::{version, NAME, PACKAGE};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

const RELEASES: &str = "https://github.com/ai-ecoverse/slicc-serve/releases/latest/download";

pub fn run(out: &mut dyn std::io::Write, err: &mut dyn std::io::Write) -> i32 {
    if std::env::var("SCSERVE_LAUNCHER").as_deref() == Ok("npm") {
        let _ = writeln!(out, "npm install -g {PACKAGE}@latest");
        return 0;
    }
    match upgrade() {
        Ok(new) if new == version() => {
            let _ = writeln!(out, "{NAME}: already up to date ({new})");
            0
        }
        Ok(new) => {
            let _ = writeln!(out, "{NAME}: {} → {new}", version());
            0
        }
        Err(e) => {
            let _ = writeln!(err, "{NAME}: {e}");
            1
        }
    }
}

fn upgrade() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or("can't tell where scserve is installed")?
        .to_path_buf();
    let base = std::env::var("SLICC_INSTALL_BASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| RELEASES.into());
    let name = if cfg!(windows) {
        "install.ps1"
    } else {
        "install.sh"
    };
    let url = format!("{}/{name}", base.trim_end_matches('/'));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let mut response = agent.get(&url).call().map_err(|e| match e {
        ureq::Error::Timeout(_) => "download timed out".to_string(),
        other => format!("could not download {url}: {other}"),
    })?;
    if !response.status().is_success() {
        return Err(format!(
            "could not download {url}: HTTP {}",
            response.status().as_u16()
        ));
    }
    let mut script = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(4 << 20)
        .read_to_end(&mut script)
        .map_err(|e| format!("could not download {url}: {e}"))?;
    let work = std::env::temp_dir().join(format!("scserve-upgrade-{}", crate::fsx::random_hex(8)));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let file = work.join(name);
    std::fs::write(&file, &script).map_err(|e| e.to_string())?;
    let mut command = if cfg!(windows) {
        let shell = ["powershell.exe", "pwsh.exe"]
            .into_iter()
            .find(|s| {
                Command::new(s)
                    .arg("-?")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok()
            })
            .ok_or("neither powershell.exe nor pwsh.exe is on PATH")?;
        let mut c = Command::new(shell);
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&file)
        .arg("-Dir")
        .arg(&dir)
        .env_remove("PSModulePath");
        c
    } else {
        let mut c = Command::new("sh");
        c.arg(&file).arg("--dir").arg(&dir);
        c
    };
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("can't run {name}: {e}"));
    let _ = std::fs::remove_dir_all(&work);
    let output = output?;
    let text = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        return Err(format!("{name} failed ({})", output.status));
    }
    text.lines()
        .find_map(|l| {
            l.strip_prefix(&format!("{NAME} "))
                .and_then(|rest| rest.split_once(" installed to "))
                .map(|(v, _)| v.to_string())
        })
        .ok_or_else(|| format!("{name} didn't say what it installed"))
}

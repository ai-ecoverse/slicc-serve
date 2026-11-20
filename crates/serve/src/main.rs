#[cfg(target_os = "wasi")]
mod kernel;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&args);
    std::process::exit(code);
}

#[cfg(not(target_os = "wasi"))]
fn run(args: &[String]) -> i32 {
    #[cfg(windows)]
    sweep_parked();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let code = serve_hub::cli::run(args, &mut out, &mut err);
    let _ = std::io::Write::flush(&mut out);
    code
}

#[cfg(windows)]
fn sweep_parked() {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
    else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("scserve.exe.") && name.ends_with(".old") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(target_os = "wasi")]
fn run(args: &[String]) -> i32 {
    kernel::run(args)
}

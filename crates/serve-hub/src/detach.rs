use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn spawn_relay(log: &Path) -> std::io::Result<u32> {
    let exe = std::env::current_exe()?;
    if let Some(dir) = log.parent() {
        crate::fsx::private_dir(dir)?;
    }
    let log = File::create(log)?;
    let mut command = Command::new(exe);
    command
        .arg("__relay")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    detach(&mut command);
    let child = command.spawn()?;
    Ok(child.id())
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        unsafe {
            let handle = GetStdHandle(which);
            if !handle.is_null() {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut Command) {}

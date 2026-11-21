use std::ffi::CString;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::FromRawFd;

mod ffi {
    #[link(wasm_import_module = "wasix_32v1")]
    extern "C" {
        pub fn fd_pipe(r: *mut u32, w: *mut u32) -> u16;
        pub fn fd_dup(fd: u32, out: *mut u32) -> u16;
        pub fn proc_id(out: *mut u32) -> u16;
        pub fn proc_signal(pid: u32, sig: u32) -> u16;
        pub fn proc_spawn3(
            name: *const u8,
            name_len: u32,
            argv: *const *const u8,
            argc: u32,
            envs: *const *const u8,
            envc: u32,
            ops: *const u8,
            opc: u32,
            signals: *const u8,
            nsignals: u32,
            search: u32,
            path: *const u8,
            path_len: u32,
            pid: *mut u32,
        ) -> u16;
    }

    #[link(wasm_import_module = "wasi_snapshot_preview1")]
    extern "C" {
        pub fn fd_renumber(from: u32, to: u32) -> u16;
        pub fn fd_close(fd: u32) -> u16;
    }

    #[link(wasm_import_module = "slicc")]
    extern "C" {
        pub fn sigaction_set(sig: u32, disposition: u32, flags: u32) -> u16;
    }

    #[link(wasm_import_module = "slicc_fs")]
    extern "C" {
        pub fn path_chmod(dirfd: i32, path: *const u8, len: u32, mode: u32, flags: u32) -> u16;
    }
}

pub const SIGHUP: u32 = 1;
pub const SIGTERM: u32 = 15;
const SIG_IGN: u32 = 1;
const AT_FDCWD: i32 = -2;
const OP_SIZE: usize = 56;
const OFLAG_CREAT: u16 = 1;
const FDFLAG_APPEND: u16 = 1;
const RIGHT_FD_WRITE: u64 = 1 << 6;

fn cvt(errno: u16) -> io::Result<()> {
    match errno {
        0 => Ok(()),
        e => Err(io::Error::from_raw_os_error(i32::from(e))),
    }
}

pub fn pipe() -> io::Result<(u32, u32)> {
    let (mut r, mut w) = (0, 0);
    cvt(unsafe { ffi::fd_pipe(&mut r, &mut w) })?;
    Ok((r, w))
}

pub fn dup(fd: u32) -> io::Result<u32> {
    let mut out = 0;
    cvt(unsafe { ffi::fd_dup(fd, &mut out) })?;
    Ok(out)
}

pub fn close(fd: u32) -> io::Result<()> {
    cvt(unsafe { ffi::fd_close(fd) })
}

pub fn renumber(from: u32, to: u32) -> io::Result<()> {
    cvt(unsafe { ffi::fd_renumber(from, to) })?;
    let _ = close(from);
    Ok(())
}

pub fn pid() -> u32 {
    let mut out = 0;
    unsafe { ffi::proc_id(&mut out) };
    out
}

pub fn signal(pid: u32, sig: u32) -> io::Result<()> {
    cvt(unsafe { ffi::proc_signal(pid, sig) })
}

pub fn ignore(sig: u32) -> io::Result<()> {
    cvt(unsafe { ffi::sigaction_set(sig, SIG_IGN, 0) })
}

pub fn chmod(path: &std::path::Path, mode: u32) {
    let text = path.to_string_lossy();
    unsafe { ffi::path_chmod(AT_FDCWD, text.as_ptr(), text.len() as u32, mode, 0) };
}

pub fn file(fd: u32) -> File {
    unsafe { File::from_raw_fd(fd as i32) }
}

pub enum Op {
    Close(u32),
    Null(u32),
    Dup2 { src: u32, fd: u32 },
    Append { fd: u32, path: String },
}

pub fn spawn(argv: &[String], ops: &[Op]) -> io::Result<u32> {
    let args: Vec<CString> = argv
        .iter()
        .map(|a| CString::new(a.as_str()).map_err(|_| io::Error::other("a NUL in an argument")))
        .collect::<io::Result<_>>()?;
    let pointers: Vec<*const u8> = args.iter().map(|a| a.as_ptr().cast()).collect();
    let paths: Vec<String> = ops
        .iter()
        .map(|op| match op {
            Op::Null(_) => "/dev/null".to_string(),
            Op::Append { path, .. } => path.clone(),
            Op::Dup2 { .. } | Op::Close(_) => String::new(),
        })
        .collect();
    let mut table = vec![0u8; OP_SIZE * ops.len()];
    for (i, op) in ops.iter().enumerate() {
        let slot = &mut table[i * OP_SIZE..(i + 1) * OP_SIZE];
        let path = &paths[i];
        let (cmd, fd, src) = match op {
            Op::Close(fd) => (0u8, *fd, 0),
            Op::Null(fd) => (2u8, *fd, 0),
            Op::Dup2 { src, fd } => (1u8, *fd, *src),
            Op::Append { fd, .. } => (2u8, *fd, 0),
        };
        slot[0] = cmd;
        slot[4..8].copy_from_slice(&fd.to_le_bytes());
        slot[8..12].copy_from_slice(&src.to_le_bytes());
        slot[12..16].copy_from_slice(&(path.as_ptr() as usize as u32).to_le_bytes());
        slot[16..20].copy_from_slice(&(path.len() as u32).to_le_bytes());
        if let Op::Append { .. } = op {
            slot[24..26].copy_from_slice(&OFLAG_CREAT.to_le_bytes());
            slot[32..40].copy_from_slice(&RIGHT_FD_WRITE.to_le_bytes());
            slot[48..50].copy_from_slice(&FDFLAG_APPEND.to_le_bytes());
        }
    }
    let name = &argv[0];
    let search = std::env::var("PATH").unwrap_or_default();
    let mut pid = 0;
    cvt(unsafe {
        ffi::proc_spawn3(
            name.as_ptr(),
            name.len() as u32,
            pointers.as_ptr(),
            pointers.len() as u32,
            std::ptr::null(),
            0,
            table.as_ptr(),
            ops.len() as u32,
            std::ptr::null(),
            0,
            u32::from(!name.contains('/')),
            search.as_ptr(),
            search.len() as u32,
            &mut pid,
        )
    })?;
    Ok(pid)
}

pub fn quietly<T>(marker: &str, f: impl FnOnce() -> T) -> (T, Option<String>) {
    let Ok(saved) = dup(2) else {
        return (f(), None);
    };
    let Ok(echo) = dup(saved) else {
        let _ = close(saved);
        return (f(), None);
    };
    let Ok((r, w)) = pipe() else {
        let _ = close(saved);
        let _ = close(echo);
        return (f(), None);
    };
    if renumber(w, 2).is_err() {
        let _ = close(w);
        let _ = close(r);
        let _ = close(saved);
        let _ = close(echo);
        return (f(), None);
    }
    let marker = marker.to_string();
    let forward = std::thread::spawn(move || {
        let mut out = file(echo);
        let mut message = None;
        for line in BufReader::new(file(r)).lines() {
            let Ok(line) = line else { break };
            match line.find(&marker) {
                Some(i) if message.is_none() => {
                    message = Some(line[i + marker.len()..].to_string())
                }
                _ => {
                    let _ = writeln!(out, "{line}");
                }
            }
        }
        message
    });
    let result = f();
    let _ = renumber(saved, 2);
    let message = forward.join().ok().flatten();
    (result, message)
}

pub fn alive(pid: u32) -> bool {
    let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let first = cmdline.split(|b| *b == 0).next().unwrap_or_default();
    String::from_utf8_lossy(first).ends_with("serve")
}

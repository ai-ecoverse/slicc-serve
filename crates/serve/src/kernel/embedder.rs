use super::sys;
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::IntoRawFd;

pub const DEVICE: &str = "/dev/embedder";
pub const EACCES: i32 = 2;
pub const ENOTSUP: i32 = 58;

pub enum Error {
    Missing,
    Refused { errno: i32, message: String },
}

impl Error {
    pub fn errno(&self) -> Option<i32> {
        match self {
            Error::Missing => None,
            Error::Refused { errno, .. } => Some(*errno),
        }
    }

    pub fn lines(&self, cmd: &str) -> Vec<String> {
        match self {
            Error::Missing => vec![format!(
                "{cmd}: {DEVICE} not available (needs slicc-kernel >= 1.153.0)"
            )],
            Error::Refused { errno, message } => {
                let mut lines = vec![format!("{cmd}: {message}")];
                if *errno == EACCES {
                    lines.push(format!(
                        "{cmd}: if seven asked, answer in seven's tab (a click allows it), then run {cmd} again"
                    ));
                }
                lines
            }
        }
    }
}

fn text(errno: i32) -> &'static str {
    match errno {
        2 => "Permission denied",
        10 => "Device or resource busy",
        27 => "Interrupted system call",
        29 => "Input/output error",
        44 => "No such file or directory",
        58 => "Not supported",
        _ => "request failed",
    }
}

fn refused(err: io::Error, message: Option<String>) -> Error {
    let errno = err.raw_os_error().unwrap_or(29);
    Error::Refused {
        errno,
        message: message.unwrap_or_else(|| text(errno).to_string()),
    }
}

fn open(write_only: bool) -> Result<File, Error> {
    let mut options = OpenOptions::new();
    options.write(true).read(!write_only);
    options.open(DEVICE).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => Error::Missing,
        _ => refused(e, None),
    })
}

fn attempt<T>(f: impl FnOnce() -> io::Result<T>) -> Result<T, Error> {
    let (result, captured) = sys::quietly(f);
    let marker = format!("{DEVICE}: ");
    let mut message = None;
    for line in captured.lines() {
        match line.find(&marker) {
            Some(i) if message.is_none() => message = Some(line[i + marker.len()..].to_string()),
            _ => eprintln!("{line}"),
        }
    }
    result.map_err(|e| refused(e, message))
}

fn header(value: &Value) -> Vec<u8> {
    let mut bytes = value.to_string().into_bytes();
    bytes.push(b'\n');
    bytes
}

pub fn publish(request: &Value) -> Result<(BufReader<File>, String), Error> {
    let mut file = open(false)?;
    attempt(move || {
        file.write_all(&header(request))?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        Ok((reader, line))
    })
}

pub fn exchange(request: &Value) -> Result<String, Error> {
    let mut file = open(false)?;
    attempt(move || {
        file.write_all(&header(request))?;
        let mut reply = String::new();
        file.read_to_string(&mut reply)?;
        Ok(reply)
    })
}

pub fn tell(request: &Value) -> Result<(), Error> {
    let mut file = open(true)?;
    attempt(move || {
        file.write_all(&header(request))?;
        sys::close(file.into_raw_fd() as u32)
    })
}

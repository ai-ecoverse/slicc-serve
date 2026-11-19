use std::io::{self, Read, Write};

const MAX_HEAD: usize = 16 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub version: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn keep_alive(&self) -> bool {
        let connection = self.header("connection").unwrap_or("").to_ascii_lowercase();
        if self.version == "HTTP/1.0" {
            connection.contains("keep-alive")
        } else {
            !connection.contains("close")
        }
    }
}

#[derive(Debug)]
pub enum ReadError {
    Closed,
    Malformed,
    TooLarge,
    Io(io::Error),
}

pub fn read_request<R: Read>(reader: &mut R, carry: &mut Vec<u8>) -> Result<Request, ReadError> {
    let mut buf = [0u8; 4096];
    loop {
        if let Some(end) = find_head_end(carry) {
            let head: Vec<u8> = carry.drain(..end + 4).collect();
            return parse_head(&head[..end]);
        }
        if carry.len() > MAX_HEAD {
            return Err(ReadError::TooLarge);
        }
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ReadError::Io(e)),
        };
        if n == 0 {
            return Err(if carry.is_empty() {
                ReadError::Closed
            } else {
                ReadError::Malformed
            });
        }
        carry.extend_from_slice(&buf[..n]);
    }
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

pub fn parse_head(head: &[u8]) -> Result<Request, ReadError> {
    let text = std::str::from_utf8(head).map_err(|_| ReadError::Malformed)?;
    let mut lines = text.split("\r\n");
    let line = lines.next().ok_or(ReadError::Malformed)?;
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ReadError::Malformed);
    };
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/1.") {
        return Err(ReadError::Malformed);
    }
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(ReadError::Malformed)?;
        if name.is_empty() || name.contains(' ') {
            return Err(ReadError::Malformed);
        }
        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
    }
    Ok(Request {
        method: method.to_string(),
        target: target.to_string(),
        version: version.to_string(),
        headers,
    })
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        304 => "Not Modified",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        416 => "Range Not Satisfiable",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "Unknown",
    }
}

pub fn write_head<W: Write>(
    out: &mut W,
    status: u16,
    headers: &[(&str, String)],
) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (name, value) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    out.write_all(head.as_bytes())
}

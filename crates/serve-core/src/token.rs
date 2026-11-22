pub const HOST_ID_LEN: usize = 20;
pub const SECRET_LEN: usize = 32;

fn hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn is_token(input: &str) -> bool {
    input
        .split_once('-')
        .is_some_and(|(host, secret)| hex(host, HOST_ID_LEN) && hex(secret, SECRET_LEN))
}

pub fn from_url(input: &str) -> Option<String> {
    let rest = match input.find("://") {
        Some(i) => &input[i + 3..],
        None => input,
    };
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?.to_ascii_lowercase();
    let (label, domain) = host.split_once('.')?;
    (!domain.is_empty() && is_token(label)).then(|| label.to_string())
}

pub fn host_of(token: &str) -> &str {
    token.split('-').next().unwrap_or(token)
}

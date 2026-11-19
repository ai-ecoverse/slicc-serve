pub fn from_url(input: &str) -> Option<String> {
    let rest = match input.find("://") {
        Some(i) => &input[i + 3..],
        None => input,
    };
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?.to_ascii_lowercase();
    let (label, domain) = host.split_once('.')?;
    if domain.is_empty() {
        return None;
    }
    let (compact, secret) = label.split_once("--")?;
    if compact.len() != 32 || !compact.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let secret = match secret.split_once('-') {
        Some((hash, secret)) if hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_hexdigit()) => {
            secret
        }
        Some(_) => return None,
        None => secret,
    };
    if secret.is_empty() || !secret.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!(
        "{}-{}-{}-{}-{}.{}",
        &compact[0..8],
        &compact[8..12],
        &compact[12..16],
        &compact[16..20],
        &compact[20..],
        secret
    ))
}

pub fn is_token(input: &str) -> bool {
    let Some((tray, secret)) = input.split_once('.') else {
        return false;
    };
    let groups: Vec<&str> = tray.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
        && !secret.is_empty()
        && secret.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn tray_of(token: &str) -> &str {
    token.split('.').next().unwrap_or(token)
}

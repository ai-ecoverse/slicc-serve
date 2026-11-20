#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    Whole,
    Unsatisfiable,
    Bytes { start: u64, end: u64 },
}

pub fn parse(header: Option<&str>, size: u64) -> Range {
    let Some(header) = header else {
        return Range::Whole;
    };
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return Range::Whole;
    };
    let Some((raw_start, raw_end)) = spec.split_once('-') else {
        return Range::Whole;
    };
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !digits(raw_start) || !digits(raw_end) || (raw_start.is_empty() && raw_end.is_empty()) {
        return Range::Whole;
    }
    let num = |s: &str| s.parse::<u64>().unwrap_or(u64::MAX);
    let (start, end) = if raw_start.is_empty() {
        let suffix = num(raw_end);
        if suffix == 0 {
            return Range::Unsatisfiable;
        }
        (size.saturating_sub(suffix), size.saturating_sub(1))
    } else {
        let start = num(raw_start);
        let end = if raw_end.is_empty() {
            size.saturating_sub(1)
        } else {
            num(raw_end).min(size.saturating_sub(1))
        };
        (start, end)
    };
    if size == 0 || start >= size || start > end {
        return Range::Unsatisfiable;
    }
    Range::Bytes { start, end }
}

pub fn content_range(header: &str) -> Option<(u64, u64, u64)> {
    let rest = header.trim().strip_prefix("bytes ")?;
    let (span, total) = rest.split_once('/')?;
    let (start, end) = span.split_once('-')?;
    Some((
        start.trim().parse().ok()?,
        end.trim().parse().ok()?,
        total.trim().parse().ok()?,
    ))
}

pub fn unsatisfied_size(header: &str) -> Option<u64> {
    header.trim().strip_prefix("bytes */")?.trim().parse().ok()
}

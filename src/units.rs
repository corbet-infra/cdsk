//! Small value conversions shared by convergence and disk planning.

/// Parse a ZFS size (`"32K"`, `"1M"`, `"4096"`, `"1T"`, ...) into bytes.
///
/// ZFS reports and accepts sizes with a binary suffix; comparing a declaration
/// against an observed value by text alone would report drift forever, so both
/// sides go through this function before they are compared.
#[must_use]
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let text = text
        .strip_suffix('B')
        .or_else(|| text.strip_suffix('b'))
        .unwrap_or(text);
    let (digits, multiplier) = match text.as_bytes().last()? {
        b'K' | b'k' => (&text[..text.len() - 1], 1024_u64),
        b'M' | b'm' => (&text[..text.len() - 1], 1024_u64.pow(2)),
        b'G' | b'g' => (&text[..text.len() - 1], 1024_u64.pow(3)),
        b'T' | b't' => (&text[..text.len() - 1], 1024_u64.pow(4)),
        b'P' | b'p' => (&text[..text.len() - 1], 1024_u64.pow(5)),
        _ => (text, 1),
    };
    digits.parse::<u64>().ok()?.checked_mul(multiplier)
}

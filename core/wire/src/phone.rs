//! Phone numbers in E.164, carried as their digits without "+".

use crate::Error;

/// Reads what a person typed ("+49 170 123-4567") into E.164 digits ("491701234567").
/// The number must start with "+" or "00"; spaces, dashes, dots and brackets are ignored.
pub fn normalize(input: &str) -> Result<String, Error> {
    let trimmed = input.trim();
    let rest = if let Some(rest) = trimmed.strip_prefix('+') {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("00") {
        rest
    } else {
        return Err(Error::Phone);
    };
    let mut digits = String::with_capacity(15);
    for c in rest.chars() {
        match c {
            '0'..='9' => digits.push(c),
            ' ' | '-' | '.' | '(' | ')' => {}
            _ => return Err(Error::Phone),
        }
    }
    check(digits.as_bytes())?;
    Ok(digits)
}

/// E.164 digits: 6 to 15 ASCII digits, the first not 0.
pub fn check(digits: &[u8]) -> Result<(), Error> {
    let ok = (6..=15).contains(&digits.len())
        && digits.iter().all(u8::is_ascii_digit)
        && digits[0] != b'0';
    ok.then_some(()).ok_or(Error::Phone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize("+49 170 123-4567").unwrap(), "491701234567");
        assert_eq!(normalize("0049 (170) 1234567").unwrap(), "491701234567");
        assert_eq!(normalize("+1.415.555.0100").unwrap(), "14155550100");
    }

    #[test]
    fn refuses() {
        for bad in ["0170 1234567", "+0123456789", "+49", "+4917012345678901", "+49 170 abc", ""] {
            assert_eq!(normalize(bad), Err(Error::Phone), "{bad}");
        }
    }
}

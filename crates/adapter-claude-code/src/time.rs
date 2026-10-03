//! Minimal RFC 3339 timestamp parsing, enough for Claude Code transcripts.

/// Parses `YYYY-MM-DDTHH:MM:SS[.fraction](Z|+HH:MM|-HH:MM)` into milliseconds
/// since the Unix epoch. Returns `None` for anything else.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year = digits(s, 0, 4)?;
    let month = digits(s, 5, 7)?;
    let day = digits(s, 8, 10)?;
    let hour = digits(s, 11, 13)?;
    let minute = digits(s, 14, 16)?;
    let second = digits(s, 17, 19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    if second > 60 {
        return None;
    }

    let mut pos = 19;
    let mut millis = 0;
    if b.get(pos) == Some(&b'.') {
        pos += 1;
        let start = pos;
        while pos < b.len() && b[pos].is_ascii_digit() {
            pos += 1;
        }
        if pos == start {
            return None;
        }
        // Keep the first three fraction digits, padding short fractions.
        let frac = &s[start..pos.min(start + 3)];
        millis = frac.parse::<i64>().ok()? * 10_i64.pow(3 - frac.len() as u32);
    }

    let offset_minutes = match b.get(pos) {
        Some(b'Z' | b'z') if pos + 1 == b.len() => 0,
        Some(sign @ (b'+' | b'-')) if pos + 6 == b.len() && b[pos + 3] == b':' => {
            let h = digits(s, pos + 1, pos + 3)?;
            let m = digits(s, pos + 4, pos + 6)?;
            let total = h * 60 + m;
            if *sign == b'+' { total } else { -total }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    Some(secs * 1_000 + millis)
}

fn digits(s: &str, from: usize, to: usize) -> Option<i64> {
    let part = s.get(from..to)?;
    if !part.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil` algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::parse_rfc3339_ms;

    #[test]
    fn parses_utc_with_millis() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2026-01-01T00:00:00.000Z"),
            Some(1_767_225_600_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-01-01T09:00:50.202Z"),
            Some(1_767_225_600_000 + 9 * 3_600_000 + 50_202)
        );
    }

    #[test]
    fn parses_offsets_and_short_or_long_fractions() {
        assert_eq!(
            parse_rfc3339_ms("2026-01-01T01:00:00+01:00"),
            Some(1_767_225_600_000)
        );
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00.5Z"), Some(500));
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00.123456Z"), Some(123));
        assert_eq!(
            parse_rfc3339_ms("2024-02-29T00:00:00Z"),
            Some(1_709_164_800_000)
        );
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "",
            "2026-01-01",
            "2026-13-01T00:00:00Z",
            "2026-01-01T00:00:00",
            "2026-01-01T00:00:00.Z",
            "2026-01-01T00:00:00Zjunk",
            "not a timestamp at all!",
        ] {
            assert_eq!(parse_rfc3339_ms(bad), None, "{bad}");
        }
    }
}

//! Timestamps: parsing GitLab's ISO 8601 times and showing them as "3h".

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Seconds since 1970 for `2026-11-08T08:18:43Z`, `2026-11-08T08:18:43.123Z`
/// or `2026-11-08T09:18:43+01:00`.
pub fn parse_iso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') {
        let digits = r.chars().take_while(|c| c.is_ascii_digit()).count();
        rest = &r[digits..];
    }
    let offset = match rest {
        "" | "Z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let oh = rest.get(1..3)?.parse::<i64>().ok()?;
            let om = rest
                .get(4..6)
                .and_then(|m| m.parse::<i64>().ok())
                .unwrap_or(0);
            sign * (oh * 3600 + om * 60)
        }
    };
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se - offset)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `2026-11-08T08:18:43Z` for seconds since 1970, in UTC.
pub fn iso(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// The date of a day since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

/// Short age: `now`, `5m`, `3h`, `2d`, `4w`, `3mo`, `1y`.
pub fn age(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        86_400..=1_209_599 => format!("{}d", s / 86_400),
        1_209_600..=5_183_999 => format!("{}w", s / 604_800),
        5_184_000..=31_535_999 => format!("{}mo", s / 2_592_000),
        _ => format!("{}y", s / 31_536_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2026-11-08T08:18:43Z"), Some(1_794_125_923));
        assert_eq!(parse_iso("2026-11-08T08:18:43.517Z"), Some(1_794_125_923));
        assert_eq!(parse_iso("2026-11-08T09:18:43+01:00"), Some(1_794_125_923));
        assert_eq!(parse_iso("yesterday"), None);
    }

    #[test]
    fn formats() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_794_125_923), "2026-11-08T08:18:43Z");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
        for t in [1_794_125_922, 1_700_000_000, 86_399, 4_102_444_800] {
            assert_eq!(parse_iso(&iso(t)), Some(t));
        }
    }

    #[test]
    fn ages() {
        assert_eq!(age(10), "now");
        assert_eq!(age(300), "5m");
        assert_eq!(age(7200), "2h");
        assert_eq!(age(3 * 86_400), "3d");
        assert_eq!(age(21 * 86_400), "3w");
        assert_eq!(age(90 * 86_400), "3mo");
        assert_eq!(age(800 * 86_400), "2y");
    }
}

//! Small text helpers shared by the core (ports of the Swift `TextUtil`).
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static CTR: AtomicU64 = AtomicU64::new(0);

/// Seconds since the Unix epoch.
pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Random lowercase hex id of `n` characters.
pub fn short_id(n: usize) -> String {
    let mut s = String::new();
    while s.len() < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write_u64(CTR.fetch_add(1, Ordering::Relaxed));
        s.push_str(&format!("{:016x}", h.finish()));
    }
    s.truncate(n);
    s
}

pub fn prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Keep the head (60%) and tail of a long text (same marker text as the Swift core).
pub fn truncate_middle(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max || max <= 200 {
        return s.to_string();
    }
    let head = (max as f64 * 0.6) as usize;
    let tail = max - head;
    let omitted = n - head - tail;
    let h: String = s.chars().take(head).collect();
    let t: String = s.chars().skip(n - tail).collect();
    format!("{h}\n…[省略 {omitted} 字符 / {omitted} chars omitted]…\n{t}")
}

/// Rough token estimate: CJK ≈ 1 token/char, digits ≈ 1 token each, ASCII punctuation ≈ 1/2, other ≈ 1 token / 4 chars.
/// (Calibrated on real transcripts against the qwen tokenizer: tables of numbers and `|` were undercounted ~35 %.)
pub fn estimate_tokens(s: &str) -> usize {
    let (mut cjk, mut digit, mut punct, mut other) = (0usize, 0usize, 0usize, 0usize);
    for c in s.chars() {
        let u = c as u32;
        if (0x4E00..=0x9FFF).contains(&u) || (0x3000..=0x30FF).contains(&u) || (0xFF00..=0xFFEF).contains(&u) {
            cjk += 1
        } else if c.is_ascii_digit() {
            digit += 1
        } else if c.is_ascii_punctuation() {
            punct += 1
        } else {
            other += 1
        }
    }
    cjk + digit + (punct + 1) / 2 + other / 4 + 1
}

fn strip_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// C `printf("%.4g")`.
pub fn fmt_g4(n: f64) -> String {
    if n == 0.0 {
        return "0".into();
    }
    if !n.is_finite() {
        return format!("{n}");
    }
    let exp = n.abs().log10().floor() as i32;
    if !(-4..4).contains(&exp) {
        let s = format!("{:.3e}", n);
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let e: i32 = e.parse().unwrap_or(0);
        format!("{}e{}{:02}", strip_zeros(m), if e < 0 { '-' } else { '+' }, e.abs())
    } else {
        strip_zeros(&format!("{:.*}", (3 - exp).max(0) as usize, n))
    }
}

/// A table cell number for the model / exports: integers as they are, |n| ≥ 1 with up to 4 decimals (amounts keep
/// their cents: 3505.5688, -70980000.12 — %.4g used to print 3506 / -7.098e+07 and reconciliations lost the decimals),
/// small fractions with 4 significant digits, huge values in scientific notation.
pub fn fmt_num(n: f64) -> String {
    if !n.is_finite() || n.abs() >= 1e15 { return fmt_g4(n); }
    if n.round() == n { return format!("{}", n as i64); }
    if n.abs() >= 1.0 { return strip_zeros(&format!("{n:.4}")); }
    fmt_g4(n)
}

fn utc_offset() -> i64 { crate::platform::utc_offset() }

/// Local civil time: (year, month, day, hour, minute, second, weekday 0=Sunday).
pub fn local_time(secs: f64) -> (i64, u32, u32, u32, u32, u32, u32) {
    let t = secs as i64 + utc_offset();
    let days = t.div_euclid(86400);
    let rem = t.rem_euclid(86400);
    // civil_from_days (H. Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let wd = (days + 4).rem_euclid(7) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d, (rem / 3600) as u32, ((rem % 3600) / 60) as u32, (rem % 60) as u32, wd)
}

pub fn today_line() -> String {
    let (y, m, d, _, _, _, wd) = local_time(now());
    let names = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    format!("{y:04}-{m:02}-{d:02} {}", names[wd as usize])
}

/// Session id like the Swift store: yyyyMMdd-HHmmss-xxxxxx.
pub fn session_id() -> String {
    let (y, m, d, hh, mm, ss, _) = local_time(now());
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}-{}", short_id(6))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn g4() {
        assert_eq!(fmt_g4(3.14159), "3.142");
        assert_eq!(fmt_g4(123456.0), "1.235e+05");
        assert_eq!(fmt_g4(0.5), "0.5");
        assert_eq!(fmt_num(42.0), "42");
        assert_eq!(fmt_num(3505.568775), "3505.5688");
        assert_eq!(fmt_num(-70980000.12), "-70980000.12");
        assert_eq!(fmt_num(-70980000.0), "-70980000");
        assert_eq!(fmt_num(0.000123456), "0.0001235");
        assert_eq!(fmt_num(2.5), "2.5");
    }
    #[test]
    fn trunc() {
        let s = "a".repeat(1000);
        let t = truncate_middle(&s, 300);
        assert!(t.contains("省略 700 字符"));
        assert_eq!(estimate_tokens("中文ab"), 3);
    }
}

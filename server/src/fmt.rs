//! Number and time formats for the pages.

/// Frozen time: "41.6 s", "3 m 05 s", "2 h 07 m". The spaces are
/// no-break spaces (U+00A0), so a value never breaks inside.
pub fn dur(ms: u64) -> String {
    let s = ms as f64 / 1000.0;
    if s < 59.95 {
        return format!("{s:.1}\u{a0}s");
    }
    let total = s.round() as u64;
    if total < 3600 {
        format!("{}\u{a0}m\u{a0}{:02}\u{a0}s", total / 60, total % 60)
    } else {
        let m = (total + 30) / 60;
        format!("{}\u{a0}h\u{a0}{:02}\u{a0}m", m / 60, m % 60)
    }
}

/// One freeze: "2.31 s", "12.4 s", "1 m 02 s" (no-break spaces).
pub fn dur1(ms: u64) -> String {
    if ms < 10_000 {
        format!("{:.2}\u{a0}s", ms as f64 / 1000.0)
    } else {
        dur(ms)
    }
}

/// "72%"
pub fn pct(share: f64) -> String {
    format!("{}%", (share * 100.0).round() as i64)
}

/// Thousands with a narrow space: "1 704 220".
pub fn num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('\u{202F}');
        }
        out.push(ch);
    }
    out
}

/// "1.70 M", "48.2 k" for big counters.
pub fn big(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2} M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{:.1} k", n as f64 / 1e3)
    } else {
        num(n)
    }
}

pub fn bytes(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.2} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// Change of a count or a time: "+7%", "−12%", "new", "0%".
pub fn change(cur: f64, prev: f64) -> String {
    if prev == 0.0 {
        return if cur == 0.0 {
            "0%".into()
        } else {
            "new".into()
        };
    }
    let p = ((cur - prev) / prev * 100.0).round() as i64;
    signed(p, "%")
}

/// Change of a share in points: "+1 pt".
pub fn change_pt(cur: f64, prev: f64) -> String {
    let p = ((cur - prev) * 100.0).round() as i64;
    signed(p, " pt")
}

pub fn signed(v: i64, unit: &str) -> String {
    if v > 0 {
        format!("+{v}{unit}")
    } else if v < 0 {
        format!("\u{2212}{}{unit}", -v)
    } else {
        format!("0{unit}")
    }
}

/// Bar width as a CSS percentage of `max`.
pub fn w(part: u64, max: u64) -> String {
    if max == 0 {
        "0%".into()
    } else {
        format!("{:.1}%", part as f64 / max as f64 * 100.0)
    }
}

/// Average with one decimal, or a whole number from 10.
pub fn avg(v: f64) -> String {
    if v >= 10.0 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

/// Percent-encode a query value.
pub fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                o.push(b as char)
            }
            b' ' => o.push('+'),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(dur(41_600), "41.6\u{a0}s");
        assert_eq!(dur(185_000), "3\u{a0}m\u{a0}05\u{a0}s");
        assert_eq!(dur(7_620_000), "2\u{a0}h\u{a0}07\u{a0}m");
        assert_eq!(dur1(2_310), "2.31\u{a0}s");
        assert_eq!(num(1_704_220), "1\u{202F}704\u{202F}220");
        assert_eq!(num(12), "12");
        assert_eq!(change(57.0, 50.0), "+14%");
        assert_eq!(change(40.0, 50.0), "\u{2212}20%");
        assert_eq!(change(3.0, 0.0), "new");
        assert_eq!(change_pt(0.71, 0.70), "+1 pt");
        assert_eq!(enc("Customers | List"), "Customers+%7C+List");
        assert_eq!(pct(0.725), "73%");
    }
}

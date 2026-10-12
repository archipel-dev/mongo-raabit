//! Number formatting shared by the views: byte sizes, counts and durations.

/// Human readable byte size (`1.2 MB`).
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Compact count (`12.3k`).
pub fn count(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{:.1}k", n as f64 / 1000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Elapsed time (`12 ms`, `1.40 s`).
pub fn duration(d: std::time::Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1.0 {
        "<1 ms".into()
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

/// `1234567` → `1,234,567`.
pub fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The range a page shows, as the paging bar prints it: `21–40 of 1,234`, or
/// `No documents` when the page is empty. `first` is the 1-based index of the page's first
/// document (0 when there is none), `len` its document count, `total` the collection count
/// when known.
pub fn page_range(first: u64, len: u64, total: Option<u64>) -> String {
    if first == 0 || len == 0 {
        return "No documents".to_owned();
    }
    let last = first + len - 1;
    match total {
        Some(n) => format!("{}–{} of {}", group(first), group(last), group(n)),
        None => format!("{}–{}", group(first), group(last)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(300 * 1024 * 1024), "300 MB");
        assert_eq!(count(1200), "1200");
        assert_eq!(count(12_345), "12.3k");
        assert_eq!(count(2_500_000), "2.5M");
        assert_eq!(duration(std::time::Duration::from_micros(2500)), "2 ms");
        assert_eq!(duration(std::time::Duration::from_micros(300)), "<1 ms");
        assert_eq!(duration(std::time::Duration::from_millis(1400)), "1.40 s");
    }

    #[test]
    fn groups_digits() {
        assert_eq!(group(0), "0");
        assert_eq!(group(1200), "1,200");
        assert_eq!(group(1_234_567), "1,234,567");
    }

    #[test]
    fn page_ranges() {
        assert_eq!(page_range(0, 0, Some(5)), "No documents");
        assert_eq!(page_range(1, 20, Some(1234)), "1–20 of 1,234");
        assert_eq!(page_range(21, 20, None), "21–40");
        assert_eq!(page_range(1, 1, Some(1)), "1–1 of 1");
    }
}

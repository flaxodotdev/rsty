use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

pub fn home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")))
}

pub fn fmt_size(b: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if b < 1024 {
        return format!("{b} B");
    }
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn fmt_count(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

pub fn fmt_age(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=3_599 => "now".into(),
        3_600..=86_399 => format!("{}h", s / 3_600),
        86_400..=2_591_999 => format!("{}d", s / 86_400),
        2_592_000..=31_535_999 => format!("{}mo", s / 2_592_000),
        _ => format!("{:.1}y", s as f64 / 31_536_000.0),
    }
}

/// Parses "100M", "1.5G", "500MB", "2GiB", "4096" (bytes). Binary units.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let num: f64 = num.parse().map_err(|_| format!("bad size: {s}"))?;
    let mult = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1u64,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        u => return Err(format!("unknown unit '{u}'")),
    };
    Ok((num * mult as f64) as u64)
}

pub fn display_path(p: &Path) -> String {
    let h = home();
    if h != Path::new("/") {
        if let Ok(rel) = p.strip_prefix(h) {
            return if rel.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rel.display())
            };
        }
    }
    p.display().to_string()
}

/// Keeps the end of a string (the interesting part of a path) within `width` chars.
pub fn shorten_left(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n <= width {
        return s.to_string();
    }
    if width <= 1 {
        return "…".into();
    }
    let tail: String = s.chars().skip(n - (width - 1)).collect();
    format!("…{tail}")
}

/// Bytes actually allocated on disk (handles sparse files and APFS clones better than len()).
#[cfg(unix)]
pub fn alloc(md: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    md.blocks() * 512
}

#[cfg(not(unix))]
pub fn alloc(md: &std::fs::Metadata) -> u64 {
    md.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("100M").unwrap(), 100 << 20);
        assert_eq!(parse_size("1.5GiB").unwrap(), 3 << 29);
        assert_eq!(parse_size("4096").unwrap(), 4096);
        assert!(parse_size("12x").is_err());
        assert_eq!(fmt_size(1536), "1.5 KiB");
        assert_eq!(fmt_size(500), "500 B");
    }

    #[test]
    fn shorten() {
        assert_eq!(shorten_left("abcdef", 4), "…def");
        assert_eq!(shorten_left("abc", 4), "abc");
    }
}

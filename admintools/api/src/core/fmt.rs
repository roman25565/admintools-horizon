//! Chat formatting shared by the commands: short numbers, bars, durations.

/// `12345` -> `12.3k`.
pub fn count(n: usize) -> String {
    if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// A 10-cell bar for a share 0..1.
pub fn bar(share: f32) -> String {
    let cells = (share.clamp(0.0, 1.0) * 10.0).round() as usize;
    format!("{}{}", "|".repeat(cells), ".".repeat(10 - cells))
}

/// A color for a tick time: green under 30 ms, yellow under 50, red above.
pub fn mspt_color(ms: f32) -> u32 {
    if ms < 30.0 {
        0x55FF55
    } else if ms < 50.0 {
        0xFFFF55
    } else {
        0xFF5555
    }
}

/// A color for TPS.
pub fn tps_color(tps: f64) -> u32 {
    if tps >= 19.5 {
        0x55FF55
    } else if tps >= 15.0 {
        0xFFFF55
    } else {
        0xFF5555
    }
}

/// Parses `on/off/true/false/1/0/так/ні`.
pub fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" | "так" => Some(true),
        "off" | "false" | "0" | "no" | "ні" => Some(false),
        _ => None,
    }
}

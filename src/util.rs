//! Small shared formatting helpers.

/// Formats a byte count using binary (IEC) units: B, KiB, MiB, GiB, TiB.
///
/// The divisor is 1024, so the unit names match the math (`1 MiB` == 1048576
/// bytes). This is the single source of truth for size formatting across the UI.
pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}

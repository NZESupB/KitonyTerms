//! 监控数值的统一展示格式。

/// 将百分比限制在可展示的 0..=100 范围内，非有限值按 0 处理。
pub(crate) fn clamp_percent(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// 计算已用容量百分比，容量未知时安全降级为 0。
pub(crate) fn percent(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        clamp_percent(used as f32 / total as f32 * 100.0)
    }
}

/// 统一使用紧凑的二进制单位；保持底部监控卡片的既有显示格式。
pub(crate) fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub(crate) fn format_rate(bytes_per_second: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_second))
}

pub(crate) fn format_uptime(seconds: u64) -> String {
    let hours = seconds / 3600;
    if hours >= 24 {
        format!("{}d {}h", hours / 24, hours % 24)
    } else {
        format!("{}h", hours)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_and_rates_use_the_same_compact_units() {
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MB");
        assert_eq!(format_rate(2 * 1024), "2.0 KB/s");
    }

    #[test]
    fn uptime_uses_compact_days_and_hours() {
        assert_eq!(format_uptime(3600), "1h");
        assert_eq!(format_uptime(25 * 3600), "1d 1h");
    }

    #[test]
    fn percent_handles_zero_and_out_of_range_values() {
        assert_eq!(percent(1, 0), 0.0);
        assert_eq!(percent(150, 100), 100.0);
        assert_eq!(clamp_percent(f32::NAN), 0.0);
    }
}

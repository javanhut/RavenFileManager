use std::time::Duration;

/// Parse a simple cron expression and return the interval as a `Duration`.
///
/// Supports a basic subset of cron patterns:
/// - `*/N * * * *` — every N minutes
/// - `0 */N * * *` — every N hours
/// - `* * * * *` — every minute
/// - `0 * * * *` — every hour
///
/// Returns `None` if the expression cannot be parsed into a simple interval.
pub fn parse_cron_interval(cron_expr: &str) -> Option<Duration> {
    let parts: Vec<&str> = cron_expr.split_whitespace().collect();
    if parts.len() != 5 {
        return None;
    }

    let minute = parts[0];
    let hour = parts[1];

    // "* * * * *" — every minute
    if minute == "*" && hour == "*" {
        return Some(Duration::from_secs(60));
    }

    // "*/N * * * *" — every N minutes
    if let Some(n_str) = minute.strip_prefix("*/") {
        if hour == "*" {
            if let Ok(n) = n_str.parse::<u64>() {
                if n > 0 {
                    return Some(Duration::from_secs(n * 60));
                }
            }
        }
        return None;
    }

    // "0 */N * * *" — every N hours
    if minute == "0" {
        if let Some(n_str) = hour.strip_prefix("*/") {
            if let Ok(n) = n_str.parse::<u64>() {
                if n > 0 {
                    return Some(Duration::from_secs(n * 3600));
                }
            }
            return None;
        }

        // "0 * * * *" — every hour
        if hour == "*" {
            return Some(Duration::from_secs(3600));
        }
    }

    // "N * * * *" — a specific minute of every hour => every hour
    if hour == "*" {
        if minute.parse::<u64>().is_ok() {
            return Some(Duration::from_secs(3600));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_minute() {
        let d = parse_cron_interval("* * * * *");
        assert_eq!(d, Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_every_5_minutes() {
        let d = parse_cron_interval("*/5 * * * *");
        assert_eq!(d, Some(Duration::from_secs(300)));
    }

    #[test]
    fn test_every_15_minutes() {
        let d = parse_cron_interval("*/15 * * * *");
        assert_eq!(d, Some(Duration::from_secs(900)));
    }

    #[test]
    fn test_every_30_minutes() {
        let d = parse_cron_interval("*/30 * * * *");
        assert_eq!(d, Some(Duration::from_secs(1800)));
    }

    #[test]
    fn test_every_hour() {
        let d = parse_cron_interval("0 * * * *");
        assert_eq!(d, Some(Duration::from_secs(3600)));
    }

    #[test]
    fn test_every_2_hours() {
        let d = parse_cron_interval("0 */2 * * *");
        assert_eq!(d, Some(Duration::from_secs(7200)));
    }

    #[test]
    fn test_every_6_hours() {
        let d = parse_cron_interval("0 */6 * * *");
        assert_eq!(d, Some(Duration::from_secs(21600)));
    }

    #[test]
    fn test_every_12_hours() {
        let d = parse_cron_interval("0 */12 * * *");
        assert_eq!(d, Some(Duration::from_secs(43200)));
    }

    #[test]
    fn test_every_24_hours() {
        let d = parse_cron_interval("0 */24 * * *");
        assert_eq!(d, Some(Duration::from_secs(86400)));
    }

    #[test]
    fn test_specific_minute_each_hour() {
        // "30 * * * *" means "at minute 30 of every hour" => treat as every hour
        let d = parse_cron_interval("30 * * * *");
        assert_eq!(d, Some(Duration::from_secs(3600)));
    }

    #[test]
    fn test_invalid_empty() {
        assert_eq!(parse_cron_interval(""), None);
    }

    #[test]
    fn test_invalid_too_few_fields() {
        assert_eq!(parse_cron_interval("* *"), None);
    }

    #[test]
    fn test_invalid_too_many_fields() {
        assert_eq!(parse_cron_interval("* * * * * *"), None);
    }

    #[test]
    fn test_invalid_non_numeric() {
        assert_eq!(parse_cron_interval("*/abc * * * *"), None);
    }

    #[test]
    fn test_zero_interval_rejected() {
        assert_eq!(parse_cron_interval("*/0 * * * *"), None);
    }

    #[test]
    fn test_zero_hour_interval_rejected() {
        assert_eq!(parse_cron_interval("0 */0 * * *"), None);
    }

    #[test]
    fn test_unsupported_complex_pattern() {
        // Specific day-of-week patterns are not supported
        assert_eq!(parse_cron_interval("0 9 * * 1"), None);
    }

    #[test]
    fn test_every_1_minute() {
        let d = parse_cron_interval("*/1 * * * *");
        assert_eq!(d, Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_every_1_hour() {
        let d = parse_cron_interval("0 */1 * * *");
        assert_eq!(d, Some(Duration::from_secs(3600)));
    }
}

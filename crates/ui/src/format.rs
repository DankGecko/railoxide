use std::time::Duration;

const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: u64 = 24 * SECONDS_PER_HOUR;
const SECONDS_PER_MONTH: u64 = 30 * SECONDS_PER_DAY;
const SECONDS_PER_YEAR: u64 = 365 * SECONDS_PER_DAY;

pub fn format_compact_latency(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds == 0 {
        return format!("{}ms", duration.subsec_millis());
    }
    if seconds < SECONDS_PER_MINUTE {
        return format!("{}.{:01}s", seconds, duration.subsec_millis() / 100);
    }

    let minutes = seconds / SECONDS_PER_MINUTE;
    if minutes < SECONDS_PER_MINUTE {
        return format!("{minutes}m");
    }

    let hours = seconds / SECONDS_PER_HOUR;
    if hours < 100 {
        return format!("{hours}h");
    }
    "99+h".to_owned()
}

pub fn format_compact_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < SECONDS_PER_MINUTE {
        return format!("{seconds}s");
    }
    if seconds < SECONDS_PER_HOUR {
        return format!("{}m", seconds / SECONDS_PER_MINUTE);
    }
    if seconds < 3 * SECONDS_PER_HOUR {
        return format_parts(
            seconds / SECONDS_PER_HOUR,
            "h",
            seconds % SECONDS_PER_HOUR / SECONDS_PER_MINUTE,
            "m",
        );
    }
    if seconds < SECONDS_PER_DAY {
        return format!("{}h", seconds / SECONDS_PER_HOUR);
    }
    if seconds < 3 * SECONDS_PER_DAY {
        return format_parts(
            seconds / SECONDS_PER_DAY,
            "d",
            seconds % SECONDS_PER_DAY / SECONDS_PER_HOUR,
            "h",
        );
    }
    if seconds < SECONDS_PER_MONTH {
        return format!("{}d", seconds / SECONDS_PER_DAY);
    }
    if seconds < 3 * SECONDS_PER_MONTH {
        return format_parts(
            seconds / SECONDS_PER_MONTH,
            "mo",
            seconds % SECONDS_PER_MONTH / SECONDS_PER_DAY,
            "d",
        );
    }
    if seconds < SECONDS_PER_YEAR {
        return format!("{}mo", seconds / SECONDS_PER_MONTH);
    }

    let years = seconds / SECONDS_PER_YEAR;
    if years >= 100 {
        return "99+y".to_owned();
    }
    if years < 3 {
        return format_parts(
            years,
            "y",
            seconds % SECONDS_PER_YEAR / SECONDS_PER_MONTH,
            "mo",
        );
    }
    format!("{years}y")
}

pub fn format_relative_age(age: Duration) -> String {
    format!("{} ago", format_compact_duration(age))
}

fn format_parts(primary: u64, primary_unit: &str, secondary: u64, secondary_unit: &str) -> String {
    if secondary == 0 {
        format!("{primary}{primary_unit}")
    } else {
        format!("{primary}{primary_unit} {secondary}{secondary_unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_duration_covers_adaptive_boundaries_and_caps() {
        let minute = Duration::from_secs(SECONDS_PER_MINUTE);
        let hour = Duration::from_secs(SECONDS_PER_HOUR);
        let day = Duration::from_secs(SECONDS_PER_DAY);
        let month = Duration::from_secs(SECONDS_PER_MONTH);
        let year = Duration::from_secs(SECONDS_PER_YEAR);

        assert_eq!(format_compact_duration(Duration::ZERO), "0s");
        assert_eq!(format_compact_duration(Duration::from_secs(59)), "59s");
        assert_eq!(format_compact_duration(minute), "1m");
        assert_eq!(
            format_compact_duration(Duration::from_secs(59 * 60 + 59)),
            "59m"
        );
        assert_eq!(format_compact_duration(hour), "1h");
        assert_eq!(
            format_compact_duration(Duration::from_secs(2 * 3_600 + 14 * 60)),
            "2h 14m"
        );
        assert_eq!(
            format_compact_duration(Duration::from_secs(3 * 3_600)),
            "3h"
        );
        assert_eq!(
            format_compact_duration(Duration::from_secs(23 * 3_600 + 59 * 60)),
            "23h"
        );
        assert_eq!(format_compact_duration(day), "1d");
        assert_eq!(
            format_compact_duration(Duration::from_secs(2 * 86_400 + 3 * 3_600)),
            "2d 3h"
        );
        assert_eq!(
            format_compact_duration(Duration::from_secs(3 * 86_400)),
            "3d"
        );
        assert_eq!(
            format_compact_duration(Duration::from_secs(29 * 86_400)),
            "29d"
        );
        assert_eq!(format_compact_duration(month), "1mo");
        assert_eq!(
            format_compact_duration(Duration::from_secs(2 * SECONDS_PER_MONTH + 4 * 86_400)),
            "2mo 4d"
        );
        assert_eq!(
            format_compact_duration(Duration::from_secs(3 * SECONDS_PER_MONTH)),
            "3mo"
        );
        assert_eq!(
            format_compact_duration(Duration::from_secs(11 * SECONDS_PER_MONTH)),
            "11mo"
        );
        assert_eq!(format_compact_duration(year), "1y");
        assert_eq!(
            format_compact_duration(Duration::from_secs(
                2 * SECONDS_PER_YEAR + 3 * SECONDS_PER_MONTH
            )),
            "2y 3mo"
        );
        assert_eq!(
            format_compact_duration(Duration::from_secs(3 * SECONDS_PER_YEAR)),
            "3y"
        );
        assert_eq!(
            format_compact_duration(Duration::from_secs(99 * SECONDS_PER_YEAR)),
            "99y"
        );
        assert_eq!(
            format_compact_duration(Duration::from_secs(100 * SECONDS_PER_YEAR)),
            "99+y"
        );
        assert_eq!(format_compact_duration(Duration::MAX), "99+y");
    }

    #[test]
    fn compact_latency_covers_boundaries_and_caps() {
        assert_eq!(format_compact_latency(Duration::from_millis(999)), "999ms");
        assert_eq!(format_compact_latency(Duration::from_millis(1_250)), "1.2s");
        assert_eq!(format_compact_latency(Duration::from_secs(60)), "1m");
        assert_eq!(
            format_compact_latency(Duration::from_secs(SECONDS_PER_HOUR)),
            "1h"
        );
        assert_eq!(
            format_compact_latency(Duration::from_secs(100 * SECONDS_PER_HOUR)),
            "99+h"
        );
        assert_eq!(format_compact_latency(Duration::MAX), "99+h");
    }

    #[test]
    fn relative_age_appends_suffix_and_remains_bounded() {
        assert_eq!(format_relative_age(Duration::from_secs(2 * 60)), "2m ago");
        assert_eq!(format_relative_age(Duration::MAX), "99+y ago");
    }
}

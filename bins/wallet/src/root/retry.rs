use std::time::Duration;

pub(super) const fn retry_backoff_delay(attempt: u8) -> Duration {
    Duration::from_secs(match attempt {
        0 => 1,
        1 => 2,
        2 => 4,
        _ => 5,
    })
}

pub(super) fn retry_backoff_delay_capped(
    attempt: u8,
    remaining: Option<Duration>,
) -> Option<Duration> {
    let delay = retry_backoff_delay(attempt);
    let Some(remaining) = remaining else {
        return Some(delay);
    };
    if remaining == Duration::ZERO {
        None
    } else if remaining < delay {
        Some(remaining)
    } else {
        Some(delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_maps_attempts_to_one_two_four_then_five_seconds() {
        assert_eq!(retry_backoff_delay(0), Duration::from_secs(1));
        assert_eq!(retry_backoff_delay(1), Duration::from_secs(2));
        assert_eq!(retry_backoff_delay(2), Duration::from_secs(4));
        assert_eq!(retry_backoff_delay(3), Duration::from_secs(5));
        assert_eq!(retry_backoff_delay(u8::MAX), Duration::from_secs(5));
    }

    #[test]
    fn retry_backoff_caps_to_remaining_expiry() {
        assert_eq!(
            retry_backoff_delay_capped(2, Some(Duration::from_secs(3))),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            retry_backoff_delay_capped(1, Some(Duration::from_secs(2))),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            retry_backoff_delay_capped(0, Some(Duration::from_secs(7))),
            Some(Duration::from_secs(1))
        );
    }

    #[test]
    fn retry_backoff_does_not_schedule_after_expiry() {
        assert_eq!(retry_backoff_delay_capped(0, Some(Duration::ZERO)), None);
        assert_eq!(
            retry_backoff_delay_capped(0, None),
            Some(Duration::from_secs(1))
        );
    }
}

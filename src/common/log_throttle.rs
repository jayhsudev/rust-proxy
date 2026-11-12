//! Token-window throttle for repetitive warnings.
//!
//! Connection errors and authentication failures are triggered by remote input:
//! without a throttle a client can write an arbitrary number of log records and
//! fill the disk. Each category may emit `per_window` records per second; the
//! rest are counted and reported as a single summary line in the next window.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Default number of records allowed per category per second.
pub const DEFAULT_PER_WINDOW: u32 = 5;

/// Categories are throttled independently so one noisy class cannot mask another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogCategory {
    ConnectionError,
    AuthFailure,
}

struct Window {
    start: Instant,
    allowed: u32,
    suppressed: u64,
}

pub struct LogThrottle {
    per_window: u32,
    window: Duration,
    state: Mutex<HashMap<LogCategory, Window>>,
}

impl LogThrottle {
    pub fn new(per_window: u32, window: Duration) -> Self {
        Self {
            per_window,
            window,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Returns `Some(suppressed)` when the record should be written, where
    /// `suppressed` is how many records of that category were dropped in the
    /// previous window (0 when nothing was suppressed).
    ///
    /// Returns `None` when the record must be dropped silently. A `per_window`
    /// of 0 disables throttling.
    pub fn allow(&self, category: LogCategory) -> Option<u64> {
        if self.per_window == 0 {
            return Some(0);
        }

        let now = Instant::now();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let entry = state.entry(category).or_insert(Window {
            start: now,
            allowed: 0,
            suppressed: 0,
        });

        if now.duration_since(entry.start) >= self.window {
            let suppressed = entry.suppressed;
            entry.start = now;
            entry.allowed = 1;
            entry.suppressed = 0;
            return Some(suppressed);
        }

        if entry.allowed < self.per_window {
            entry.allowed += 1;
            return Some(0);
        }

        entry.suppressed += 1;
        None
    }
}

static GLOBAL: OnceLock<LogThrottle> = OnceLock::new();

/// Installs the process-wide throttle. Later calls are ignored.
pub fn configure(per_window: u32) {
    let _ = GLOBAL.set(LogThrottle::new(per_window, Duration::from_secs(1)));
}

fn global() -> &'static LogThrottle {
    GLOBAL.get_or_init(|| LogThrottle::new(DEFAULT_PER_WINDOW, Duration::from_secs(1)))
}

/// Throttles a record using the process-wide configuration.
pub fn allow(category: LogCategory) -> Option<u64> {
    global().allow(category)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allows_up_to_the_limit() {
        let throttle = LogThrottle::new(3, Duration::from_secs(60));

        for _ in 0..3 {
            assert_eq!(throttle.allow(LogCategory::ConnectionError), Some(0));
        }
        assert_eq!(throttle.allow(LogCategory::ConnectionError), None);
    }

    #[test]
    fn test_categories_are_independent() {
        let throttle = LogThrottle::new(1, Duration::from_secs(60));

        assert_eq!(throttle.allow(LogCategory::ConnectionError), Some(0));
        assert_eq!(throttle.allow(LogCategory::ConnectionError), None);
        // A different category still has its own budget.
        assert_eq!(throttle.allow(LogCategory::AuthFailure), Some(0));
    }

    #[test]
    fn test_suppressed_count_is_reported_in_the_next_window() {
        let throttle = LogThrottle::new(1, Duration::from_millis(30));

        assert_eq!(throttle.allow(LogCategory::AuthFailure), Some(0));
        for _ in 0..4 {
            assert_eq!(throttle.allow(LogCategory::AuthFailure), None);
        }

        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(throttle.allow(LogCategory::AuthFailure), Some(4));
        assert_eq!(throttle.allow(LogCategory::AuthFailure), None);
    }

    #[test]
    fn test_zero_disables_throttling() {
        let throttle = LogThrottle::new(0, Duration::from_secs(60));

        for _ in 0..1000 {
            assert_eq!(throttle.allow(LogCategory::ConnectionError), Some(0));
        }
    }
}

//! Opt-in diagnostics.
//!
//! Diagnostics go to stderr so that stdout stays pipeable data. Each line is
//! stamped to millisecond precision, which is what makes the timings between
//! successive queries readable.

use std::fmt::Display;
use std::io::Write;

use jiff::Zoned;

#[derive(Debug, Clone, Copy)]
pub struct Diag {
    enabled: bool,
}

impl Diag {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn log(&self, message: impl Display) {
        if self.enabled {
            let _ = writeln!(
                std::io::stderr(),
                "[{}] debug: {message}",
                stamp(&Zoned::now())
            );
        }
    }
}

/// Renders the wall-clock time of a diagnostic as `HH:MM:SS.mmm`.
fn stamp(now: &Zoned) -> String {
    let time = now.time();
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        time.hour(),
        time.minute(),
        time.second(),
        time.millisecond()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_carry_milliseconds() {
        let now: Zoned = "2026-09-15T09:07:03.042+02:00[Europe/Oslo]".parse().unwrap();
        assert_eq!(stamp(&now), "09:07:03.042");
    }

    #[test]
    fn stamps_pad_every_field() {
        let now: Zoned = "2026-09-15T23:59:59.999+02:00[Europe/Oslo]".parse().unwrap();
        assert_eq!(stamp(&now), "23:59:59.999");
        let midnight: Zoned = "2026-09-15T00:00:00+02:00[Europe/Oslo]".parse().unwrap();
        assert_eq!(stamp(&midnight), "00:00:00.000");
    }
}

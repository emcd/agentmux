use agentmux::adapter::lifecycle::{Clock, ManualClock, SystemClock};

/// Manual time starts at the constructed reading and moves only through
/// explicit advances, so identical traces replay identically.
#[test]
fn manual_clock_advances_only_on_advance() {
    let clock = ManualClock::new(1_000);
    assert_eq!(clock.now_ms(), 1_000);
    clock.advance(250);
    assert_eq!(clock.now_ms(), 1_250);
}

/// Advancing past the maximum saturates instead of wrapping, so a large
/// jump never rewinds deadline arithmetic.
#[test]
fn manual_clock_saturates_at_maximum() {
    let clock = ManualClock::new(u64::MAX - 1);
    clock.advance(10);
    assert_eq!(clock.now_ms(), u64::MAX);
}

/// Distinct manual clocks are independent: advancing one leaves the
/// other untouched.
#[test]
fn manual_clocks_are_independent() {
    let first = ManualClock::new(0);
    let second = ManualClock::new(0);
    first.advance(5);
    assert_eq!(first.now_ms(), 5);
    assert_eq!(second.now_ms(), 0);
}

/// The system clock reports a non-decreasing reading.
#[test]
fn system_clock_is_non_decreasing() {
    let clock = SystemClock;
    let first = clock.now_ms();
    let second = clock.now_ms();
    assert!(second >= first);
}

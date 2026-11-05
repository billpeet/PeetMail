//! Hybrid logical clocks for settings sync: the later stamp wins a conflict. A stamp is
//! `<ms since 1970, 13 digits>:<counter, 4 digits>:<node>`, so stamps compare as strings.
//! It never goes backwards on one device and always passes the newest stamp the device has
//! seen, so a PC whose clock is behind still wins with an edit made after it synced.

/// Older than any real stamp: a setting that has never been edited (a default), which
/// loses to anything edited anywhere.
pub const ZERO: &str = "0000000000000:0000:0";

/// The next stamp after `latest` (the newest this device has made or seen), at `now_ms`.
pub fn next(latest: Option<&str>, now_ms: i64, node: &str) -> String {
    let (last_ms, last_count) = latest.and_then(parse).unwrap_or((0, 0));
    let (ms, count) = if now_ms > last_ms { (now_ms, 0) } else { (last_ms, last_count + 1) };
    format!("{ms:013}:{count:04}:{node}")
}

fn parse(stamp: &str) -> Option<(i64, u32)> {
    let mut parts = stamp.splitn(3, ':');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// A stamp from another device is usable: well-formed and not absurdly far in the future
/// (a day), which would freeze every later edit behind it.
pub fn plausible(stamp: &str, now_ms: i64) -> bool {
    let shaped = stamp.len() >= 20 && stamp.as_bytes()[13] == b':' && stamp.as_bytes()[18] == b':';
    shaped && parse(stamp).is_some_and(|(ms, _)| ms <= now_ms + 86_400_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_order_as_strings() {
        let a = next(None, 1_000, "a");
        let b = next(Some(&a), 2_000, "a");
        assert!(ZERO < a.as_str() && a < b, "{a} {b}");
        assert_eq!(a, "0000000001000:0000:a");
    }

    #[test]
    fn a_clock_that_goes_back_or_stands_still_still_moves_forward() {
        let a = next(None, 5_000, "a");
        let b = next(Some(&a), 4_000, "a");
        let c = next(Some(&b), 5_000, "a");
        assert!(a < b && b < c, "{a} {b} {c}");
        assert_eq!(b, "0000000005000:0001:a");
    }

    #[test]
    fn an_edit_after_seeing_another_device_beats_it_even_with_a_slow_clock() {
        let theirs = next(None, 9_000_000, "b");
        let mine = next(Some(&theirs), 1_000, "a");
        assert!(mine > theirs);
    }

    #[test]
    fn ties_break_by_node() {
        assert!(next(None, 1, "b") > next(None, 1, "a"));
    }

    #[test]
    fn implausible_stamps_are_refused() {
        let now = 1_700_000_000_000;
        assert!(plausible(&next(None, now, "a"), now));
        assert!(plausible(ZERO, now));
        assert!(!plausible(&next(None, now + 2 * 86_400_000, "a"), now));
        assert!(!plausible("yesterday", now));
        assert!(!plausible("", now));
    }
}

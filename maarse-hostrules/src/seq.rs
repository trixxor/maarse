//! Sequence numbers per conversation (`content::SEQ`): the sender numbers
//! its messages to each contact from 1, so the receiver can show a gap
//! ("2 messages missing") instead of passing it silently. Used between
//! nodes that announce `appdata::CAP_SEQ` (hosts). maarse-host keeps the
//! counters on disk (`node/seq.rs`); the simulator keeps them in memory.

/// What message number `n` means after the last number `last` seen from
/// the same sender (0 = none yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// The first message we see from this sender: no gap can be told.
    First,
    /// The next one in order.
    InOrder,
    /// `missing` earlier messages have not arrived (yet).
    Gap(u32),
    /// Older than the last one: late (counted as missing before) or a repeat.
    LateOrRepeat,
}

/// Classifies number `n` after `last`.
pub fn check(last: u32, n: u32) -> Seen {
    if n <= last {
        Seen::LateOrRepeat
    } else if last == 0 {
        Seen::First
    } else if n == last + 1 {
        Seen::InOrder
    } else {
        Seen::Gap(n - last - 1)
    }
}

/// What a gap looks like in the conversation.
pub fn gap_notice(missing: u32) -> String {
    if missing == 1 {
        "⚠ 1 earlier message from this contact has not arrived (it may still be on its way)".into()
    } else {
        format!("⚠ {missing} earlier messages from this contact have not arrived (they may still be on their way)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaps_and_late_messages() {
        assert_eq!(check(0, 5), Seen::First);
        assert_eq!(check(4, 5), Seen::InOrder);
        assert_eq!(check(4, 7), Seen::Gap(2));
        assert_eq!(check(7, 6), Seen::LateOrRepeat);
        assert_eq!(check(7, 7), Seen::LateOrRepeat);
        assert!(gap_notice(1).contains("1 earlier message "));
        assert!(gap_notice(3).contains("3 earlier messages"));
    }
}

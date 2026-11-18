//! Post office and backbone timings as one editable profile.
//!
//! [`Rules::STANDARD`] (also `Rules::default()`) holds the values the host
//! runs with today, the profile "Standard (as built)". The simulator can
//! load, copy and change profiles (serde); `maarse-host` always uses the
//! standard.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    /// A message without an acknowledgement within this time is put in the
    /// recipient's mailbox.
    pub deposit_after_ms: u64,
    /// The mailbox keeps at most this many messages per recipient ...
    pub mailbox_per_recipient: usize,
    /// ... for this long.
    pub mailbox_ttl_ms: u64,
    /// A recipient counts as present this long after a sync.
    pub mail_present_ms: u64,
    /// The replicas are asked for a recipient we hear at most this often.
    pub mail_fetch_every_ms: u64,
    /// First retry of a delivery from the post office (then doubling).
    pub mail_retry_first_ms: u64,
    /// Retries after the first before waiting for the recipient to ask.
    pub mail_retry_steps: u32,
    /// How often the DHT is maintained (bootstrap, publications, expiry).
    pub dht_every_ms: u64,
    /// The same address is looked up at most this often.
    pub dht_lookup_every_ms: u64,
    /// Bordercast: a second request for the same destination within this
    /// time goes on the radio anyway.
    pub bordercast_repeat_ms: u64,
    /// Key requests are answered from the announce cache at most this often
    /// per destination.
    pub key_answer_every_ms: u64,
}

impl Rules {
    /// "Standard (as built)": today's host.
    pub const STANDARD: Rules = Rules {
        deposit_after_ms: maarse_core::mail::DEPOSIT_AFTER_MS,
        mailbox_per_recipient: 100,
        mailbox_ttl_ms: 7 * 24 * 3600 * 1000,
        mail_present_ms: 600_000,
        mail_fetch_every_ms: 600_000,
        mail_retry_first_ms: 300_000,
        mail_retry_steps: 3,
        dht_every_ms: 5_000,
        dht_lookup_every_ms: 30_000,
        bordercast_repeat_ms: 120_000,
        key_answer_every_ms: 30_000,
    };

    /// The name of the standard profile.
    pub const STANDARD_NAME: &'static str = "Standard (as built)";

    /// After retry number `step` (1, 2, …) is due: the delay to the next one,
    /// or `None` when the post office stops retrying. Without the backbone
    /// the standard gives 5, then 10, 20 and 40 minutes (attempts at 5, 15,
    /// 35 and 75 minutes after the deposit).
    pub fn mail_retry_after(&self, step: u32) -> Option<u64> {
        (step <= self.mail_retry_steps).then(|| self.mail_retry_first_ms.saturating_mul(1u64 << step.min(63)))
    }
}

impl Default for Rules {
    fn default() -> Self {
        Rules::STANDARD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_is_todays_host() {
        let r = Rules::default();
        assert_eq!(r, Rules::STANDARD);
        assert_eq!(r.deposit_after_ms, 45_000);
        assert_eq!((r.mailbox_per_recipient, r.mailbox_ttl_ms), (100, 604_800_000));
        assert_eq!((r.mail_present_ms, r.mail_fetch_every_ms, r.mail_retry_first_ms), (600_000, 600_000, 300_000));
        assert_eq!((r.dht_every_ms, r.dht_lookup_every_ms), (5_000, 30_000));
        assert_eq!((r.bordercast_repeat_ms, r.key_answer_every_ms), (120_000, 30_000));
    }

    #[test]
    fn retry_doubles_then_stops() {
        let r = Rules::STANDARD;
        assert_eq!(r.mail_retry_after(1), Some(600_000));
        assert_eq!(r.mail_retry_after(2), Some(1_200_000));
        assert_eq!(r.mail_retry_after(3), Some(2_400_000));
        assert_eq!(r.mail_retry_after(4), None);
        let more = Rules { mail_retry_steps: 100, ..r };
        assert_eq!(more.mail_retry_after(80), Some(u64::MAX), "no overflow");
    }

    #[test]
    fn profiles_roundtrip_and_fill_missing_fields() {
        let r = Rules { dht_every_ms: 7_000, ..Rules::STANDARD };
        let j = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Rules>(&j).unwrap(), r);
        let partial: Rules = serde_json::from_str(r#"{"mailbox_per_recipient": 5}"#).unwrap();
        assert_eq!(partial, Rules { mailbox_per_recipient: 5, ..Rules::STANDARD });
    }
}

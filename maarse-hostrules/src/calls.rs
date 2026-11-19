//! Calls: call data goes only over the internet all the way. A route that
//! is LoRa (or unknown) anywhere is refused with `NOT_OVER_TCP`.

use crate::filebox::PathKind;
use maarse_core::companion::error_code;

/// The text the client gets with a refused call.
pub const REFUSED_TEXT: &str = "Calls require a connection over the internet all the way";

/// Why a call along a route of type `kind` is refused (error code and
/// text), or `None` when it may go.
pub fn call_refusal(kind: PathKind) -> Option<(u8, &'static str)> {
    (kind != PathKind::Tcp).then_some((error_code::NOT_OVER_TCP, REFUSED_TEXT))
}

/// The call test service (maarse-host `node/calltest.rs`): the ring's mode
/// byte and the service's timing, shared with the simulator.
pub mod test {
    /// Music (`music.bin`, looped until the caller hangs up); the default.
    pub const MUSIC: u8 = 1;
    /// Echo: what the caller says comes back after [`ECHO_DELAY_MS`].
    pub const ECHO: u8 = 2;
    /// Speed: [`SPEED_PACKETS`] full packets per [`PACKET_MS`] for
    /// [`SPEED_FOR_MS`], then the service hangs up.
    pub const SPEED: u8 = 3;
    /// One packet carries three 20 ms Opus frames.
    pub const PACKET_MS: u64 = 60;
    pub const ECHO_DELAY_MS: u64 = 1_000;
    pub const SPEED_FOR_MS: u64 = 5_000;
    /// Packets per 60 ms in the speed test (about 17 KB/s of payload).
    pub const SPEED_PACKETS: usize = 8;
    /// A call nobody hangs up ends by itself.
    pub const MAX_CALL_MS: u64 = 600_000;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tcp_all_the_way() {
        assert_eq!(call_refusal(PathKind::Tcp), None);
        assert_eq!(call_refusal(PathKind::Lora), Some((error_code::NOT_OVER_TCP, REFUSED_TEXT)));
        assert_eq!(call_refusal(PathKind::Unknown).map(|r| r.0), Some(error_code::NOT_OVER_TCP));
    }

    #[test]
    fn matches_the_file_limit_route() {
        // The host used to refuse when the file limit went via LoRa: the same rule.
        let r = maarse_core::companion::RadioConfig::EU_DEFAULT;
        for k in [PathKind::Tcp, PathKind::Lora, PathKind::Unknown] {
            for on in [true, false] {
                assert_eq!(call_refusal(k).is_some(), crate::filebox::limit(k, 1 << 20, on, &r).via_lora);
            }
        }
    }
}

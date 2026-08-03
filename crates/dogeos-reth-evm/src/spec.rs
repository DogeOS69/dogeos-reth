use dogeos_hardforks::{DogeosHardfork, DogeosHardforks};
use revm_scroll::ScrollSpecId;

/// Maps the retained Feynman+ hardfork schedule to `revm-scroll` specification IDs.
pub fn spec_id_at_timestamp_and_number(
    timestamp: u64,
    number: u64,
    chain_spec: impl DogeosHardforks,
) -> ScrollSpecId {
    let active = |fork| {
        chain_spec
            .dogeos_fork_activation(fork)
            .active_at_timestamp_or_number(timestamp, number)
    };
    if active(DogeosHardfork::Tsuki) {
        ScrollSpecId::TSUKI
    } else if active(DogeosHardfork::GalileoV2) || active(DogeosHardfork::Galileo) {
        ScrollSpecId::GALILEO
    } else {
        ScrollSpecId::FEYNMAN
    }
}

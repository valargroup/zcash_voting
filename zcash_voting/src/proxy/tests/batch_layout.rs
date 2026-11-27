//! A batch uses the single-share layout exactly when it is planned inside the
//! round's last-moment window, the same call casts use.

use super::fixtures::host_at;
use crate::proxy::{batch_layout, DcShareLayout};

/// The single-share layout applies from the window's start up to vote end,
/// and nowhere else.
#[test]
fn the_single_share_layout_is_used_exactly_inside_the_last_moment_window() {
    // A 1,000-second round has a 400-second window starting at 1,600.
    for (now, expected) in [
        (1_000, DcShareLayout::Standard),
        (1_599, DcShareLayout::Standard),
        (1_600, DcShareLayout::SingleShare),
        (1_999, DcShareLayout::SingleShare),
        (2_000, DcShareLayout::Standard),
    ] {
        let host = host_at(now, Some(1_000), Some(2_000));
        assert_eq!(
            host.is_last_moment(),
            expected == DcShareLayout::SingleShare
        );
        assert_eq!(batch_layout(&host), expected, "at {now}");
    }
}

/// Without both authenticated round times there is no window, so the layout
/// is standard.
#[test]
fn without_authenticated_timing_a_batch_uses_the_standard_layout() {
    assert_eq!(
        batch_layout(&host_at(1_999, None, Some(2_000))),
        DcShareLayout::Standard
    );
    assert_eq!(
        batch_layout(&host_at(1_999, Some(1_000), None)),
        DcShareLayout::Standard
    );
}

//! Slot positions: each bundle's DCs take that batch's first slots, below
//! eight, in an order drawn from the CSPRNG at commit.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

use super::fixtures::{bundles, planning_case, request, seeded_rng};
use crate::proxy::{plan_allocation, planner::assign_dc_slots, DcShareLayout, MAX_DC_SLOTS};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// A bundle with `k` DCs uses exactly slots `0..k`, every slot is below
    /// eight, and each slot keeps its DC's delegate and weight and the plan's
    /// layout, with no nullifier yet.
    #[test]
    fn every_bundle_s_dcs_take_its_first_slots(
        (request, weights, layout) in planning_case(),
        slot_seed in any::<u64>(),
    ) {
        // Refusals are covered by the planner properties.
        let Ok(plan) = plan_allocation(&request, &bundles(&weights), layout) else {
            return Ok(());
        };
        let slots = assign_dc_slots(&plan, &mut seeded_rng(slot_seed));

        prop_assert_eq!(slots.len(), plan.dcs().len());
        let mut slots_by_bundle: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
        for slot in &slots {
            prop_assert!(slot.dc_slot < MAX_DC_SLOTS);
            prop_assert_eq!(slot.layout, plan.layout());
            prop_assert_eq!(slot.dc_slot_nullifier, None);
            slots_by_bundle
                .entry(slot.bundle_index)
                .or_default()
                .push(slot.dc_slot);
        }
        for positions in slots_by_bundle.values() {
            let first_slots: Vec<u8> = (0..positions.len() as u8).collect();
            prop_assert_eq!(positions, &first_slots);
        }

        let mut planned: Vec<(u32, u32, u64)> = plan
            .dcs()
            .iter()
            .map(|dc| (dc.bundle_index, dc.delegate_index, dc.weight_ballots))
            .collect();
        let mut placed: Vec<(u32, u32, u64)> = slots
            .iter()
            .map(|slot| (slot.bundle_index, slot.delegate_index, slot.weight_ballots))
            .collect();
        planned.sort_unstable();
        placed.sort_unstable();
        prop_assert_eq!(planned, placed);
    }
}

/// Slot order follows the CSPRNG, not the fill order: across seeds, a batch of
/// three DCs takes every one of the six orders.
#[test]
fn slot_order_is_drawn_rather_than_fixed() {
    let plan = plan_allocation(
        &request(&[(1, 5_000), (2, 3_000), (3, 2_000)]),
        &bundles(&[100]),
        DcShareLayout::Standard,
    )
    .unwrap();
    let orders: BTreeSet<Vec<u32>> = (0..64)
        .map(|seed| {
            assign_dc_slots(&plan, &mut seeded_rng(seed))
                .iter()
                .map(|slot| slot.delegate_index)
                .collect()
        })
        .collect();
    assert_eq!(orders.len(), 6, "{orders:?}");
}

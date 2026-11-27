//! Slot positions. A bundle's `k` DCs take slots `0..k` in a CSPRNG order
//! drawn at commit, so batch order does not reveal which delegate got more
//! (PRV-9). The slot is also the circuit slot, below
//! [`MAX_DC_SLOTS`](crate::proxy::MAX_DC_SLOTS).

use std::collections::BTreeMap;

use rand::{seq::SliceRandom, CryptoRng, RngCore};

use super::{AllocationPlan, PlannedDc, ProxySlot};

/// Places each DC of `plan` at a uniformly random slot of its bundle's batch,
/// with the plan's layout and no nullifier. Returns slots ordered by bundle,
/// then slot.
pub(crate) fn assign_dc_slots<R: RngCore + CryptoRng>(
    plan: &AllocationPlan,
    rng: &mut R,
) -> Vec<ProxySlot> {
    let mut by_bundle: BTreeMap<u32, Vec<&PlannedDc>> = BTreeMap::new();
    for dc in plan.dcs() {
        by_bundle.entry(dc.bundle_index).or_default().push(dc);
    }

    let mut slots = Vec::with_capacity(plan.dc_count());
    for (bundle_index, dcs) in by_bundle {
        // Packing gives a bundle at most one DC per delegate, so at most
        // eight, which fits the circuit's 3-bit slot.
        let slot_count =
            u8::try_from(dcs.len()).expect("a bundle carries at most MAX_PROXY_DELEGATES DCs");
        let mut positions: Vec<u8> = (0..slot_count).collect();
        positions.shuffle(rng);
        for (dc, dc_slot) in dcs.into_iter().zip(positions) {
            slots.push(ProxySlot {
                bundle_index,
                dc_slot,
                delegate_index: dc.delegate_index,
                weight_ballots: dc.weight_ballots,
                layout: plan.layout(),
                dc_slot_nullifier: None,
            });
        }
    }
    slots.sort_by_key(|slot| (slot.bundle_index, slot.dc_slot));
    slots
}

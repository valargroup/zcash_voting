//! Confirm and the pre-proving check (contract §11.11), the one registry
//! check a commit relies on. See [`check_delegates`] for the verdict order.

use zcash_vote_delegate::{DelegateStatus, ProxyDelegateEntry, VerifiedWalletList};

use super::{
    listing::{identity_provider, listing},
    proofs::proven_value,
    DelegateEntryError, ProvenDelegateEntry, ProvenRecord, RegistryProofRequest,
};
use crate::wire::{
    DelegateCheck, ProxyAllocationRequest, ProxyDelegateChoice, ProxyDelegateListing,
};

/// What the check shows the proven entries with.
#[derive(Clone, Copy, Debug)]
pub struct ConfirmContext<'a> {
    /// The newest verified wallet list, for the listings.
    pub wallet_list: Option<&'a VerifiedWalletList>,
    /// The round's `next_delegate_index_at_creation`, which labels entries
    /// new this round, when the host knows it.
    pub first_new_index: Option<u32>,
}

/// One choice at confirm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmedDelegate {
    /// The verdict a commit takes.
    pub check: DelegateCheck,
    /// The confirm listing from the proven entry, `None` unless it is proven
    /// and decodes.
    pub listing: Option<ProxyDelegateListing>,
}

/// The entries to prove in one evidence answer for `request`: each distinct
/// chosen entry. The RPC server learns the chosen delegates, as helpers do.
/// Tor hides the device's address.
pub fn confirm_proof_requests(request: &ProxyAllocationRequest) -> Vec<RegistryProofRequest> {
    let mut requests = Vec::with_capacity(request.choices.len());
    for choice in &request.choices {
        let proof = RegistryProofRequest {
            delegate_index: choice.delegate_index,
        };
        if !requests.contains(&proof) {
            requests.push(proof);
        }
    }
    requests
}

/// Checks each choice of `request` against `proven`, the evidence for
/// [`confirm_proof_requests`], in request order. An entry the evidence lacks
/// counts as unproven. Each choice gets the first verdict that applies:
///
/// 1. entry unproven, or its bytes are not entry `delegate_index`:
///    `CannotVerify`
/// 2. entry proven absent: `NotFound`
/// 3. the entry's key, provider or handle is not the choice's: `Mismatch`
/// 4. `Suspended` or `Retired`
/// 5. otherwise `Ok`
pub fn check_delegates(
    context: &ConfirmContext<'_>,
    request: &ProxyAllocationRequest,
    proven: &[ProvenRecord],
) -> Vec<ConfirmedDelegate> {
    let wallet_list = context.wallet_list.map(VerifiedWalletList::list);
    request
        .choices
        .iter()
        .map(|choice| {
            let proof = proven_value(proven, choice.delegate_index);
            let (check, entry) = match ProvenDelegateEntry::from_proof(choice.delegate_index, proof)
            {
                Ok(Some(entry)) => (verdict(choice, entry.entry()), Some(entry)),
                Ok(None) => (DelegateCheck::NotFound, None),
                Err(
                    DelegateEntryError::Unavailable
                    | DelegateEntryError::Undecodable(_)
                    | DelegateEntryError::WrongIndex { .. },
                ) => (DelegateCheck::CannotVerify, None),
            };
            ConfirmedDelegate {
                check,
                listing: entry
                    .map(|entry| listing(entry.entry(), wallet_list, context.first_new_index)),
            }
        })
        .collect()
}

/// Steps 3 to 5 of [`check_delegates`] for a proven entry.
fn verdict(choice: &ProxyDelegateChoice, entry: &ProxyDelegateEntry) -> DelegateCheck {
    let picked = entry.dk.encode() == choice.dk
        && entry.provider == identity_provider(choice.provider)
        && entry.handle == choice.handle;
    if !picked {
        return DelegateCheck::Mismatch;
    }
    match entry.status {
        DelegateStatus::Active => DelegateCheck::Ok,
        DelegateStatus::Suspended => DelegateCheck::Suspended,
        DelegateStatus::Retired => DelegateCheck::Retired,
    }
}

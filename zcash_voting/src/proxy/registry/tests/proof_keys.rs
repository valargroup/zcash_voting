//! Classifying a proven store key and the values it admits (contract §11.6).

use zcash_vote_delegate::delegate_entry_store_key;

use crate::proxy::registry::{RegistryProofRequest, MAX_REGISTRY_ENTRY_BYTES};

#[test]
fn an_entry_request_round_trips_through_its_store_key() {
    for delegate_index in [1, 17, u32::MAX] {
        let request = RegistryProofRequest { delegate_index };
        assert_eq!(
            request.store_key(),
            delegate_entry_store_key(delegate_index)
        );
        assert_eq!(
            RegistryProofRequest::from_store_key(&request.store_key()),
            Some(request)
        );
    }
}

#[test]
fn keys_the_reader_never_proves_are_refused() {
    let mut used_key_prefix = delegate_entry_store_key(1);
    used_key_prefix[1] = 0x02;
    let governance_nullifier = [[1, 0].as_slice(), &[9; 64]].concat();
    for key in [
        Vec::new(),
        vec![0x1a],
        delegate_entry_store_key(0).to_vec(),
        used_key_prefix.to_vec(),
        governance_nullifier,
        delegate_entry_store_key(1)[..5].to_vec(),
        [delegate_entry_store_key(1).as_slice(), &[0]].concat(),
    ] {
        assert_eq!(
            RegistryProofRequest::from_store_key(&key),
            None,
            "{key:02x?}"
        );
    }
}

#[test]
fn an_entry_admits_one_byte_to_its_size_cap() {
    let entry = RegistryProofRequest { delegate_index: 1 };
    assert!(entry.admits(&[1]));
    assert!(entry.admits(&vec![1; MAX_REGISTRY_ENTRY_BYTES]));
    assert!(!entry.admits(&vec![1; MAX_REGISTRY_ENTRY_BYTES + 1]));
    assert!(!entry.admits(&[]));
}

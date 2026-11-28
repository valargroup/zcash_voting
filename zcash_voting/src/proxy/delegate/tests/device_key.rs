//! The device's one delegate key per vote chain: recorded by its public key
//! only, and bound to its entry by a proven entry. Key text encodings are
//! pinned by `zcash_vote_delegate`'s vectors.

use zcash_vote_delegate::DelegateStatus;

use super::fixtures::{
    alice_entry, alice_key, bob_key, chain, db, entry, proven, signing_key, ALICE_SECRET,
    BOB_SECRET, DELEGATE_INDEX,
};
use crate::{
    proxy::delegate::{
        device_delegate_key, forget_delegate_key, record_delegate_key, record_proven_entry,
        DelegateChain, DelegateModeError, DeviceDelegateKey,
    },
    types::Network,
};

/// A device holds one key per chain: a repeat keeps the first record,
/// another key is refused until the first is forgotten, and each chain of
/// each network has its own.
#[test]
fn a_device_holds_one_key_per_chain() {
    let db = db();
    let chain = chain();
    assert_eq!(device_delegate_key(&db, &chain).unwrap(), None);
    let first = record_delegate_key(&db, &chain, &alice_key(), 10).unwrap();
    assert_eq!(
        first,
        DeviceDelegateKey {
            dk: alice_key(),
            delegate_index: None,
            created_at: 10,
        }
    );
    assert_eq!(
        record_delegate_key(&db, &chain, &alice_key(), 99).unwrap(),
        first
    );
    assert!(matches!(
        record_delegate_key(&db, &chain, &bob_key(), 11),
        Err(DelegateModeError::AnotherKeyHeld { held }) if held == alice_key().encode()
    ));
    assert_eq!(device_delegate_key(&db, &chain).unwrap(), Some(first));

    for other in [
        DelegateChain::new(Network::Mainnet, "svote-1").unwrap(),
        DelegateChain::new(Network::Testnet, "zvote-1").unwrap(),
    ] {
        assert_eq!(device_delegate_key(&db, &other).unwrap(), None);
        record_delegate_key(&db, &other, &bob_key(), 12).unwrap();
    }
    assert_eq!(device_delegate_key(&db, &chain).unwrap(), Some(first));

    forget_delegate_key(&db, &chain).unwrap();
    forget_delegate_key(&db, &chain).unwrap();
    assert_eq!(device_delegate_key(&db, &chain).unwrap(), None);
    assert_eq!(
        record_delegate_key(&db, &chain, &bob_key(), 13).unwrap().dk,
        bob_key()
    );
}

/// A proven entry that holds the device's key gives the record its index,
/// once. An entry with another key, or another index for the same key, is
/// not the device's.
#[test]
fn a_proven_entry_binds_the_device_key_to_its_index() {
    let db = db();
    let chain = chain();
    assert!(matches!(
        record_proven_entry(&db, &chain, &alice_entry()),
        Err(DelegateModeError::NotDeviceEntry {
            delegate_index: DELEGATE_INDEX
        })
    ));
    record_delegate_key(&db, &chain, &alice_key(), 10).unwrap();
    let bound = record_proven_entry(&db, &chain, &alice_entry()).unwrap();
    assert_eq!(bound.delegate_index, Some(DELEGATE_INDEX));
    assert_eq!(
        record_proven_entry(&db, &chain, &alice_entry()).unwrap(),
        bound
    );
    assert_eq!(device_delegate_key(&db, &chain).unwrap(), Some(bound));

    let retired = proven(&entry(DELEGATE_INDEX, alice_key(), DelegateStatus::Retired));
    assert_eq!(record_proven_entry(&db, &chain, &retired).unwrap(), bound);

    for other in [
        entry(DELEGATE_INDEX, bob_key(), DelegateStatus::Active),
        entry(DELEGATE_INDEX + 1, alice_key(), DelegateStatus::Active),
    ] {
        assert!(
            matches!(
                record_proven_entry(&db, &chain, &proven(&other)),
                Err(DelegateModeError::NotDeviceEntry { delegate_index }) if delegate_index == other.index
            ),
            "{other:?}"
        );
    }
}

/// The device's key record holds the public key and timestamps only: no
/// secret seed and no `zvdsk1…` string reaches the database.
#[test]
fn device_records_hold_no_secret() {
    let db = db();
    let chain = chain();
    let alice = signing_key(ALICE_SECRET);
    record_delegate_key(&db, &chain, &alice.delegate_key(), 10).unwrap();
    record_proven_entry(&db, &chain, &alice_entry()).unwrap();
    let stage = DelegateChain::new(Network::Mainnet, "svote-1").unwrap();
    let bob = signing_key(BOB_SECRET);
    record_delegate_key(&db, &stage, &bob.delegate_key(), 11).unwrap();

    let dump: Vec<Vec<u8>> = {
        let conn = db.conn();
        let mut statement = conn.prepare("SELECT * FROM proxy_delegate_keys").unwrap();
        let columns = statement.column_count();
        statement
            .query_map([], |row| {
                let mut bytes = Vec::new();
                for column in 0..columns {
                    match row.get_ref(column)? {
                        rusqlite::types::ValueRef::Blob(blob) => bytes.extend_from_slice(blob),
                        rusqlite::types::ValueRef::Text(text) => bytes.extend_from_slice(text),
                        other => bytes.extend_from_slice(format!("{other:?}").as_bytes()),
                    }
                }
                Ok(bytes)
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let secrets: Vec<Vec<u8>> = [&alice, &bob]
        .iter()
        .flat_map(|key| [key.secret_seed().to_vec(), key.export().as_bytes().to_vec()])
        .collect();
    for row in &dump {
        for secret in &secrets {
            assert!(
                !row.windows(secret.len())
                    .any(|window| window == secret.as_slice()),
                "a delegate key record holds a secret"
            );
        }
    }
    assert_eq!(dump.len(), 2);
}

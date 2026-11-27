//! The device's one delegate key per vote chain: created from the OS RNG,
//! exported and imported as `zvdsk1…`, recorded by its public key only, and
//! bound to its entry by a proven entry.

use rand::{rngs::StdRng, SeedableRng};
use zcash_vote_delegate::{DelegateSigningKey, DelegateStatus, EntryDecodeError};

use super::fixtures::{
    alice_entry, alice_key, bob_key, chain, db, entry, proven, signing_key, ALICE_SECRET,
    BOB_SECRET, DELEGATE_INDEX,
};
use crate::{
    proxy::{
        delegate::{
            create_delegate_key, delegate_key_info, delegate_marker_text, device_delegate_key,
            forget_delegate_key, record_delegate_key, record_proven_entry, DelegateChain,
            DelegateEntryError, DelegateModeError, DeviceDelegateKey, ProvenDelegateEntry,
        },
        registry::ProvenValue,
    },
    types::Network,
};

/// A created key is 32 fresh bytes from the RNG, and its `zvdsk1…` export is
/// the one string the host stores and imports back.
#[test]
fn a_created_key_exports_and_imports_as_one_string() {
    let mut rng = StdRng::seed_from_u64(7);
    let created = create_delegate_key(&mut rng);
    let other = create_delegate_key(&mut rng);
    assert_ne!(created.delegate_key(), other.delegate_key());

    let stored = created.export();
    assert!(stored.starts_with("zvdsk1"));
    let imported = DelegateSigningKey::import(&stored).unwrap();
    assert_eq!(imported.secret_seed(), created.secret_seed());
    assert!(DelegateSigningKey::import(&stored.to_uppercase()).is_err());
    assert!(DelegateSigningKey::import(&created.delegate_key().encode()).is_err());

    let info = delegate_key_info(&created.delegate_key());
    assert_eq!(info.dk, created.delegate_key().encode());
    assert_eq!(
        info.fingerprint,
        created.delegate_key().fingerprint().to_string()
    );
    assert_eq!(
        delegate_marker_text(&created.delegate_key()),
        format!("zcash-vote-delegate v1 {}", info.dk)
    );
}

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

/// A proven entry is the bytes at the key that was asked for, decoded
/// strictly: absence is `None`, and anything else is refused.
#[test]
fn a_proven_entry_is_the_entry_that_was_asked_for() {
    let alice = entry(DELEGATE_INDEX, alice_key(), DelegateStatus::Active);
    let present = ProvenValue::Present(alice.encode());
    let proven = ProvenDelegateEntry::from_proof(DELEGATE_INDEX, &present)
        .unwrap()
        .unwrap();
    assert_eq!(proven.entry(), &alice);
    assert_eq!(
        ProvenDelegateEntry::from_proof(DELEGATE_INDEX, &ProvenValue::Absent),
        Ok(None)
    );
    assert_eq!(
        ProvenDelegateEntry::from_proof(DELEGATE_INDEX, &ProvenValue::Unavailable),
        Err(DelegateEntryError::Unavailable)
    );
    assert_eq!(
        ProvenDelegateEntry::from_proof(DELEGATE_INDEX + 1, &present),
        Err(DelegateEntryError::WrongIndex {
            expected: DELEGATE_INDEX + 1,
            actual: DELEGATE_INDEX,
        })
    );
    let mut truncated = alice.encode();
    truncated.pop();
    assert_eq!(
        ProvenDelegateEntry::from_proof(DELEGATE_INDEX, &ProvenValue::Present(truncated)),
        Err(DelegateEntryError::Undecodable(EntryDecodeError::Truncated))
    );
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

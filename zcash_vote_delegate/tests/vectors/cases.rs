//! Expected outputs as pure functions of each case's inputs, computed through
//! the crate's public API. [`recompute`] rebuilds the whole file from its
//! inputs, so the committed file cannot drift from the crate.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use zcash_vote_delegate::{
    are_lookalikes, attest_digest, ballot_digest, dc_seed, delegate_ballot_store_key,
    delegate_entry_store_key, delegate_handle_store_key, fold_handle, hotkey_round_key,
    is_canonical_handle, is_canonical_proof_id, osa_distance, pool_share_count_store_key,
    proxy_batch_digest, proxy_batch_sighash, register_digest, retire_digest,
    used_delegate_key_store_key, validate_ed25519_key, verify_wallet_list, verify_zip215,
    AttestationWindow, DcSeedInputs, DcShareLayout, DelegateBallotFields, DelegateKey,
    DelegateMarker, DelegateRegisterFields, DelegateRetireFields, DelegateSigningKey,
    IdentityProvider, PinnedListKey, ProxyChainError, ProxyDelegateEntry,
    ProxyDelegationActionFields, ProxyDelegationBatchFields, WalletList, WalletListRow,
    ZcashNetwork, NEXT_DELEGATE_INDEX_STORE_KEY, PROXY_DELEGATION_DOMAINS, PROXY_PARAMS_STORE_KEY,
    VOTE_CODESPACE, WALLET_LIST_FORMAT,
};

use super::{bytes, decode_hex, error_name, hex32, hex_of, model::*};

/// The file with every `expected` recomputed from its `inputs`, and the
/// domain and error tables taken from the crate.
pub fn recompute(file: &VectorFile) -> VectorFile {
    let domains: Vec<String> = file
        .domains
        .existing
        .iter()
        .cloned()
        .chain(PROXY_DELEGATION_DOMAINS.map(str::to_owned))
        .collect();
    VectorFile {
        format: file.format.clone(),
        contract: file.contract.clone(),
        generator: file.generator.clone(),
        d1: section(&file.d1, d1),
        handle: section(&file.handle, handle),
        proof_id: section(&file.proof_id, proof_id),
        register: section(&file.register, register),
        attest: section(&file.attest, attest),
        retire: section(&file.retire, retire),
        ballot: section(&file.ballot, ballot),
        domains: DomainsSection {
            existing: file.domains.existing.clone(),
            proxy_delegation: PROXY_DELEGATION_DOMAINS.map(str::to_owned).to_vec(),
            prefix_free: prefix_free(&domains),
        },
        kv_keys: section(&file.kv_keys, kv_key),
        entry: section(&file.entry, entry),
        ed25519_keys: Ed25519Section {
            key_rule: section(&file.ed25519_keys.key_rule, key_rule),
            zip215: section(&file.ed25519_keys.zip215, zip215),
        },
        delegate_key: DelegateKeySection {
            encoding: section(&file.delegate_key.encoding, key_encoding),
            decode: section(&file.delegate_key.decode, key_decode),
            import: section(&file.delegate_key.import, key_import),
        },
        dc_secrets: section(&file.dc_secrets, dc_secrets),
        marker: section(&file.marker, marker),
        lookalike: LookalikeSection {
            fold: section(&file.lookalike.fold, fold),
            pairs: section(&file.lookalike.pairs, pair),
        },
        config_domains: ConfigDomainsSection {
            domains: file.config_domains.domains.clone(),
            prefix_free: prefix_free(&file.config_domains.domains),
        },
        wallet_list_payload: section(&file.wallet_list_payload, wallet_list_payload),
        wallet_list_document: section(&file.wallet_list_document, wallet_list_document),
        errors: ProxyChainError::ALL
            .into_iter()
            .map(|error| ErrorRow {
                name: error.name().to_owned(),
                code: error.code(),
                codespace: VOTE_CODESPACE.to_owned(),
            })
            .collect(),
    }
}

/// A section's cases with `expected` recomputed.
fn section<I: Clone, E>(cases: &[Case<I, E>], recompute: impl Fn(&I) -> E) -> Vec<Case<I, E>> {
    cases
        .iter()
        .map(|case| Case {
            name: case.name.clone(),
            inputs: case.inputs.clone(),
            expected: recompute(&case.inputs),
        })
        .collect()
}

/// D1 and the batch identity for one batch.
fn d1(inputs: &D1Inputs) -> D1Expected {
    let batch = ProxyDelegationBatchFields {
        vote_round_id: hex32(&inputs.vote_round_id),
        vote_comm_tree_anchor_height: inputs.vote_comm_tree_anchor_height,
        expiry_height: inputs.expiry_height,
        registration_van_cmx: inputs.registration_van_cmx.as_deref().map(hex32),
        dcs: inputs
            .dcs
            .iter()
            .map(|dc| ProxyDelegationActionFields {
                r_vpk: hex32(&dc.r_vpk),
                van_nullifier: hex32(&dc.van_nullifier),
                vote_authority_note_new: hex32(&dc.vote_authority_note_new),
                proxy_delegation_commitment: hex32(&dc.proxy_delegation_commitment),
                dc_slot_nullifier: hex32(&dc.dc_slot_nullifier),
            })
            .collect(),
    };
    D1Expected {
        d1: hex_of(&proxy_batch_sighash(&inputs.chain_id, &batch).unwrap()),
        batch_digest: hex_of(&proxy_batch_digest(&inputs.chain_id, &batch).unwrap()),
    }
}

/// Whether a handle is canonical. An unknown provider has no canonical handle.
fn handle(inputs: &HandleInputs) -> CanonicalExpected {
    CanonicalExpected {
        canonical: IdentityProvider::from_code(inputs.provider)
            .is_some_and(|provider| is_canonical_handle(provider, &inputs.handle)),
    }
}

/// Whether a proof id is canonical. An unknown provider has none.
fn proof_id(inputs: &ProofIdInputs) -> CanonicalExpected {
    CanonicalExpected {
        canonical: IdentityProvider::from_code(inputs.provider)
            .is_some_and(|provider| is_canonical_proof_id(provider, &inputs.proof_id)),
    }
}

/// REGISTER for the key derived from `dk_sk`, signed by that key.
fn register(inputs: &RegisterInputs) -> SignedDigestExpected {
    let signing_key = DelegateSigningKey::from_secret_seed(hex32(&inputs.dk_sk));
    let fields = DelegateRegisterFields {
        dk: signing_key.delegate_key().to_bytes(),
        provider: provider(inputs.provider),
        handle: &inputs.handle,
        proof_id: &inputs.proof_id,
    };
    signed_digest(
        &signing_key,
        register_digest(&inputs.chain_id, &fields).unwrap(),
    )
}

/// ATTEST for one registration, signed by the verifier key.
fn attest(inputs: &AttestInputs) -> AttestExpected {
    let fields = DelegateRegisterFields {
        dk: hex32(&inputs.dk),
        provider: provider(inputs.provider),
        handle: &inputs.handle,
        proof_id: &inputs.proof_id,
    };
    let window = AttestationWindow {
        not_before: inputs.not_before,
        expires_at: inputs.expires_at,
    };
    let digest = attest_digest(&inputs.chain_id, &fields, &window).unwrap();
    let (verifier_key, signing_key) = signer(&inputs.verifier_secret_seed);
    AttestExpected {
        digest: hex_of(&digest),
        verifier_key,
        sig: hex_of(&signing_key.sign(&digest).to_bytes()),
    }
}

/// RETIRE of the entry whose key derives from `dk_sk`, signed by that key.
fn retire(inputs: &RetireInputs) -> SignedDigestExpected {
    let signing_key = DelegateSigningKey::from_secret_seed(hex32(&inputs.dk_sk));
    let fields = DelegateRetireFields {
        delegate_index: inputs.delegate_index,
        dk: signing_key.delegate_key().to_bytes(),
    };
    signed_digest(
        &signing_key,
        retire_digest(&inputs.chain_id, &fields).unwrap(),
    )
}

/// BALLOT signed by the key derived from `dk_sk`.
fn ballot(inputs: &BallotInputs) -> SignedDigestExpected {
    let signing_key = DelegateSigningKey::from_secret_seed(hex32(&inputs.dk_sk));
    let fields = DelegateBallotFields {
        vote_round_id: hex32(&inputs.vote_round_id),
        delegate_index: inputs.delegate_index,
        options: inputs.options.clone(),
    };
    signed_digest(
        &signing_key,
        ballot_digest(&inputs.chain_id, &fields).unwrap(),
    )
}

/// Whether no string in `items` is a prefix of another, equal ones included.
pub fn prefix_free(items: &[String]) -> bool {
    items.iter().enumerate().all(|(i, shorter)| {
        items
            .iter()
            .enumerate()
            .all(|(j, longer)| i == j || !longer.starts_with(shorter.as_str()))
    })
}

/// One store key.
fn kv_key(inputs: &KvKeyInputs) -> KvKeyExpected {
    let round_id = || hex32(inputs.vote_round_id.as_deref().unwrap());
    let key = match inputs.kind.as_str() {
        "entry" => delegate_entry_store_key(inputs.delegate_index.unwrap()).to_vec(),
        "used_key" => used_delegate_key_store_key(&hex32(inputs.dk.as_deref().unwrap())).to_vec(),
        "handle" => delegate_handle_store_key(
            provider(inputs.provider.unwrap()),
            inputs.handle.as_deref().unwrap(),
        ),
        "next_index" => NEXT_DELEGATE_INDEX_STORE_KEY.to_vec(),
        "params" => PROXY_PARAMS_STORE_KEY.to_vec(),
        "ballot" => delegate_ballot_store_key(&round_id(), inputs.delegate_index.unwrap()).to_vec(),
        "pool_share_count" => {
            pool_share_count_store_key(&round_id(), inputs.delegate_index.unwrap()).to_vec()
        }
        other => panic!("unknown store key kind {other}"),
    };
    KvKeyExpected { key: hex_of(&key) }
}

/// The entry a stored value decodes to, or why it is refused.
fn entry(inputs: &EntryInputs) -> EntryExpected {
    match ProxyDelegateEntry::decode(&decode_hex(&inputs.value)) {
        Ok(entry) => EntryExpected {
            entry: Some(EntryFields {
                index: entry.index,
                dk: hex_of(&entry.dk.to_bytes()),
                provider: entry.provider.code(),
                handle: entry.handle,
                proof_id: entry.proof_id,
                status: entry.status.code(),
                status_reason: entry.status_reason,
                registered_height: entry.registered_height,
            }),
            error: None,
        },
        Err(error) => EntryExpected {
            entry: None,
            error: Some(error_name(error)),
        },
    }
}

/// The key rule's verdict.
fn key_rule(inputs: &KeyRuleInputs) -> KeyRuleExpected {
    KeyRuleExpected {
        error: validate_ed25519_key(&hex32(&inputs.key))
            .err()
            .map(error_name),
    }
}

/// The ZIP-215 and key-rule verdicts for one signature.
fn zip215(inputs: &Zip215Inputs) -> Zip215Expected {
    let public_key = hex32(&inputs.public_key);
    let signature: [u8; 64] = bytes(&inputs.signature);
    Zip215Expected {
        error: verify_zip215(&public_key, &decode_hex(&inputs.message), &signature)
            .err()
            .map(error_name),
        key_rule_error: validate_ed25519_key(&public_key).err().map(error_name),
    }
}

/// The public key, both strings and the display of one secret seed.
fn key_encoding(inputs: &KeyEncodingInputs) -> KeyEncodingExpected {
    let signing_key = DelegateSigningKey::from_secret_seed(hex32(&inputs.dk_sk));
    let dk = signing_key.delegate_key();
    KeyEncodingExpected {
        dk: hex_of(&dk.to_bytes()),
        zvdk: dk.encode(),
        zvdsk: signing_key.export().to_string(),
        fingerprint: dk.fingerprint().to_string(),
        display: dk.fingerprint().display_with_index(inputs.delegate_index),
    }
}

/// The key a `zvdk1…` string decodes to, or why it is refused.
fn key_decode(inputs: &KeyTextInputs) -> KeyDecodeExpected {
    match DelegateKey::decode(&inputs.text) {
        Ok(dk) => KeyDecodeExpected {
            dk: Some(hex_of(&dk.to_bytes())),
            error: None,
        },
        Err(error) => KeyDecodeExpected {
            dk: None,
            error: Some(error_name(error)),
        },
    }
}

/// The secret seed a `zvdsk1…` string imports, or why it is refused.
fn key_import(inputs: &KeyTextInputs) -> KeyImportExpected {
    match DelegateSigningKey::import(&inputs.text) {
        Ok(signing_key) => KeyImportExpected {
            dk_sk: Some(hex_of(signing_key.secret_seed())),
            error: None,
        },
        Err(error) => KeyImportExpected {
            dk_sk: None,
            error: Some(error_name(error)),
        },
    }
}

/// `hrk` and `dc_seed` for one DC.
fn dc_secrets(inputs: &DcSecretsInputs) -> DcSecretsExpected {
    let network = match inputs.net_u8 {
        0 => ZcashNetwork::Mainnet,
        1 => ZcashNetwork::Testnet,
        2 => ZcashNetwork::Regtest,
        other => panic!("unknown net_u8 {other}"),
    };
    let layout = match inputs.layout {
        0 => DcShareLayout::Standard,
        1 => DcShareLayout::SingleShare,
        other => panic!("unknown layout {other}"),
    };
    let hrk = hotkey_round_key(
        &hex32(&inputs.hotkey_secret),
        network,
        &hex32(&inputs.round_id),
    );
    let seed = dc_seed(
        &hrk,
        &DcSeedInputs {
            van_nullifier: hex32(&inputs.van_nullifier),
            delegate_index: inputs.delegate_index,
            weight_ballots: inputs.weight_ballots,
            layout,
        },
    );
    DcSecretsExpected {
        hrk: hex_of(&hrk),
        dc_seed: hex_of(&seed),
    }
}

/// The parse of one post's text.
fn marker(inputs: &MarkerInputs) -> MarkerExpected {
    match DelegateMarker::parse(&inputs.text) {
        Ok(marker) => MarkerExpected {
            dk: Some(marker.dk.encode()),
            canonical_text: Some(marker.to_text()),
            error: None,
        },
        Err(error) => MarkerExpected {
            dk: None,
            canonical_text: None,
            error: Some(error_name(error)),
        },
    }
}

/// One handle's fold.
fn fold(inputs: &FoldInputs) -> FoldExpected {
    FoldExpected {
        folded: fold_handle(&inputs.handle),
    }
}

/// The folds, their distance and the lookalike verdict for two handles.
fn pair(inputs: &PairInputs) -> PairExpected {
    let fold_a = fold_handle(&inputs.a);
    let fold_b = fold_handle(&inputs.b);
    PairExpected {
        distance: osa_distance(&fold_a, &fold_b),
        lookalike: are_lookalikes(&inputs.a, &inputs.b),
        fold_a,
        fold_b,
    }
}

/// The wallet list a payload case describes; its pictures are hex.
pub fn wallet_list(inputs: &WalletListPayloadInputs) -> WalletList {
    WalletList {
        wallet_id: inputs.wallet_id.clone(),
        chain_id: inputs.chain_id.clone(),
        version: inputs.version,
        issued_at: inputs.issued_at,
        reserved_names: inputs.reserved_names.clone(),
        delegates: inputs
            .delegates
            .iter()
            .map(|row| WalletListRow {
                index: row.index,
                provider: row.provider,
                handle: row.handle.clone(),
                dk: row.dk.clone(),
                name: row.name.clone(),
                statement: row.statement.clone(),
                picture: row.picture.as_deref().map(decode_hex),
            })
            .collect(),
    }
}

/// The wallet list file as its signer publishes it.
#[derive(Serialize)]
struct PublishedWalletList {
    format: &'static str,
    wallet_id: String,
    chain_id: String,
    version: u64,
    issued_at: u64,
    reserved_names: Vec<String>,
    delegates: Vec<PublishedWalletListRow>,
    signatures: Vec<PublishedSignature>,
}

/// One published row; an absent statement or picture is omitted.
#[derive(Serialize)]
struct PublishedWalletListRow {
    index: u32,
    provider: u32,
    handle: String,
    dk: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    statement: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    picture: Option<String>,
}

/// One published signature.
#[derive(Serialize)]
struct PublishedSignature {
    key_id: String,
    alg: &'static str,
    sig: String,
}

/// The published file for `list` with `signatures` as `(key_id, signature)`:
/// pretty-printed JSON with a trailing newline.
pub fn render_wallet_list(list: &WalletList, signatures: &[(&str, [u8; 64])]) -> String {
    let published = PublishedWalletList {
        format: WALLET_LIST_FORMAT,
        wallet_id: list.wallet_id.clone(),
        chain_id: list.chain_id.clone(),
        version: list.version,
        issued_at: list.issued_at,
        reserved_names: list.reserved_names.clone(),
        delegates: list
            .delegates
            .iter()
            .map(|row| PublishedWalletListRow {
                index: row.index,
                provider: row.provider,
                handle: row.handle.clone(),
                dk: row.dk.clone(),
                name: row.name.clone(),
                statement: row.statement.clone(),
                picture: row
                    .picture
                    .as_deref()
                    .map(|picture| STANDARD.encode(picture)),
            })
            .collect(),
        signatures: signatures
            .iter()
            .map(|(key_id, sig)| PublishedSignature {
                key_id: (*key_id).to_owned(),
                alg: "ed25519",
                sig: STANDARD.encode(sig),
            })
            .collect(),
    };
    super::render(&published)
}

/// The payload of one wallet list, its signature by the case's signer, and
/// the published file; or the first rule it breaks.
fn wallet_list_payload(inputs: &WalletListPayloadInputs) -> WalletListPayloadExpected {
    let (signer_key, signing_key) = signer(&inputs.signer_secret_seed);
    let list = wallet_list(inputs);
    match list.payload() {
        Ok(payload) => {
            let signature = signing_key.sign(&payload).to_bytes();
            WalletListPayloadExpected {
                payload: Some(hex_of(&payload)),
                signer_key,
                signature: Some(hex_of(&signature)),
                document: Some(render_wallet_list(
                    &list,
                    &[(inputs.signer_key_id.as_str(), signature)],
                )),
                error: None,
            }
        }
        Err(error) => WalletListPayloadExpected {
            payload: None,
            signer_key,
            signature: None,
            document: None,
            error: Some(error.name().to_owned()),
        },
    }
}

/// A fetched wallet list's verdict under the case's pinned keys.
fn wallet_list_document(inputs: &WalletListDocumentInputs) -> WalletListDocumentExpected {
    let pinned: Vec<PinnedListKey> = inputs
        .pinned_keys
        .iter()
        .map(|key| PinnedListKey {
            key_id: key.key_id.clone(),
            public_key: hex32(&key.public_key),
        })
        .collect();
    match verify_wallet_list(
        inputs.document.as_bytes(),
        &inputs.wallet_id,
        &inputs.chain_id,
        &pinned,
    ) {
        Ok(verified) => WalletListDocumentExpected {
            payload: Some(hex_of(verified.payload())),
            signed_by: Some(verified.signed_by().to_owned()),
            error: None,
        },
        Err(error) => WalletListDocumentExpected {
            payload: None,
            signed_by: None,
            error: Some(error.name().to_owned()),
        },
    }
}

/// The provider for a vector's wire code.
fn provider(code: u32) -> IdentityProvider {
    IdentityProvider::from_code(code).expect("vectors use provider 1 or 2")
}

/// A digest with the signing key's public key and its signature.
fn signed_digest(signing_key: &DelegateSigningKey, digest: [u8; 32]) -> SignedDigestExpected {
    SignedDigestExpected {
        dk: hex_of(&signing_key.delegate_key().to_bytes()),
        digest: hex_of(&digest),
        dk_sig: hex_of(&signing_key.sign_digest(&digest)),
    }
}

/// The RFC 8032 key for a vector secret seed, as hex, and its signing key.
fn signer(secret_seed: &str) -> (String, ed25519_zebra::SigningKey) {
    let signing_key = ed25519_zebra::SigningKey::from(hex32(secret_seed));
    let public_key: [u8; 32] = ed25519_zebra::VerificationKey::from(&signing_key).into();
    (hex_of(&public_key), signing_key)
}

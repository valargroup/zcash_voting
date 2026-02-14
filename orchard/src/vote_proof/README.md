# Vote Proof Circuit (ZKP 2)

Proves that a registered voter is casting a valid vote, without revealing which VAN they hold. The structure follows the delegation circuit's pattern (ZKP 1) and implements conditions incrementally.

**Public inputs:** 7 field elements.
**Current K:** 11 (2,048 rows) — sufficient for condition 2 alone, will increase.

## Inputs

- Public (7 field elements)
   * **van_nullifier** (offset 0): the nullifier of the old VAN being spent (prevents double-vote).
   * **vote_authority_note_new** (offset 1): the new VAN commitment with decremented proposal authority.
   * **vote_commitment** (offset 2): the vote commitment hash.
   * **vote_comm_tree_root** (offset 3): root of the Poseidon-based vote commitment tree at anchor height.
   * **vote_comm_tree_anchor_height** (offset 4): the vote-chain height at which the tree is snapshotted.
   * **proposal_id** (offset 5): which proposal this vote is for.
   * **voting_round_id** (offset 6): the voting round identifier — prevents cross-round replay.

- Private (VAN ownership — conditions 1–4)
   * **voting_hotkey_pk**: the voting hotkey public key (x-coordinate of the ECC point derived from `vsk`).
   * **total_note_value**: the voter's total delegated weight.
   * **proposal_authority_old**: remaining proposal authority bitmask in the old VAN.
   * **gov_comm_rand**: blinding randomness for the VAN commitment.
   * **vote_authority_note_old**: the old VAN commitment (Poseidon hash of its components).
   * **vote_comm_tree_path**: Poseidon-based Merkle authentication path (24 sibling hashes).
   * **vote_comm_tree_position**: leaf position in the vote commitment tree.
   * **vsk**: voting spending key (scalar for ECC multiplication).
   * **vsk_nk**: nullifier deriving key derived from `vsk`.

- Private (vote commitment — conditions 7–11)
   * **shares_1..4**: the voting share vector (each in `[0, 2^24)`).
   * **share_randomness_1..4**: El Gamal encryption randomness per share.
   * **vote_decision**: the voter's choice.

- Internal wires (not public inputs, not free witnesses)
   * **voting_round_id cell**: copied from the instance column, used in condition 2 Poseidon hash.
   * **proposal_authority_new**: derived as `proposal_authority_old - 1` (condition 5).
   * **shares_hash**: derived from encrypted shares (condition 9).

## Condition 2: VAN Integrity ✅

Purpose: prove that the old VAN commitment is correctly constructed from its components. This binds the voter's identity, weight, round, authority, and blinding factor into a single authenticated leaf in the vote commitment tree.

```
vote_authority_note_old = Poseidon(DOMAIN_VAN, voting_hotkey_pk, total_note_value,
                                   voting_round_id, proposal_authority_old, gov_comm_rand)
```

Where:
- **DOMAIN_VAN**: `0`. Domain separation tag for Vote Authority Notes (vs `DOMAIN_VC = 1` for Vote Commitments). Assigned via `assign_advice_from_constant` so the value is baked into the verification key.
- **voting_hotkey_pk**: the voting hotkey public key. A single Pallas base field element (the x-coordinate of the ECC point derived from `vsk`). Shared with condition 3 (spend authority check).
- **total_note_value**: the voter's total delegated weight. Shared with condition 7 (shares sum check).
- **voting_round_id**: the vote round identifier (public input at offset 6). Copied from the instance column via `assign_advice_from_instance`, ensuring the in-circuit value matches the verifier's public input.
- **proposal_authority_old**: remaining proposal authority bitmask. Shared with condition 5 (decrement check).
- **gov_comm_rand**: random blinding factor. Prevents observers from brute-forcing the address or weight from the public VAN commitment.
- **vote_authority_note_old**: the witnessed VAN commitment. Constrained to equal the Poseidon output via `constrain_equal`.

**Function:** `Poseidon` with `ConstantLength<6>`. Uses `Pow5Chip` / `P128Pow5T3` with rate 2 (3 absorption rounds for 6 inputs).

**Constraint:** The circuit computes `derived_van = Poseidon(DOMAIN_VAN, voting_hotkey_pk, total_note_value, voting_round_id, proposal_authority_old, gov_comm_rand)` and enforces strict equality `derived_van == vote_authority_note_old`. Since `vote_authority_note_old` will also be used as the Merkle leaf in condition 1, this creates a binding: the VAN membership proof and the VAN integrity check are tied to the same commitment.

**Out-of-circuit helper:** `van_integrity_hash()` computes the same Poseidon hash outside the circuit for builder and test use.

**Constructions:** `PoseidonChip`.

## Condition 1: VAN Membership (TODO)

Purpose: prove the voter's VAN is registered in the vote commitment tree, without revealing which one.

```
MerklePath(vote_authority_note_old, vote_comm_tree_position, vote_comm_tree_path) = vote_comm_tree_root
```

Poseidon-based Merkle path verification (32 levels). Analogous to ZKP 1 condition 10.

**Constructions:** `PoseidonChip`, custom swap gate.

## Condition 3: Spend Authority (TODO)

Purpose: prove the voter controls the hotkey delegated to in Phase 1–2.

```
voting_hotkey_pk = ExtractP([vsk] * G)
```

**Constructions:** `EccChip`.

## Condition 4: VAN Nullifier Integrity (TODO)

Purpose: derive a nullifier that prevents double-voting without revealing the VAN.

```
van_nullifier = Poseidon_vsk.nk("vote authority spend", voting_round_id, vote_authority_note_old)
```

**Constructions:** `PoseidonChip`.

## Condition 5: Proposal Authority Decrement (TODO)

Purpose: ensure the voter still has authority and correctly decrements it.

```
proposal_authority_new = proposal_authority_old - 1
proposal_authority_old > 0
```

**Constructions:** `AddChip`, range check.

## Condition 6: New VAN Integrity (TODO)

Purpose: the new VAN has the same fields as the old except with decremented authority.

```
vote_authority_note_new = Poseidon(DOMAIN_VAN, voting_hotkey_pk, total_note_value,
                                   voting_round_id, proposal_authority_new, gov_comm_rand)
```

**Constructions:** `PoseidonChip`.

## Condition 7: Shares Sum Correctness (TODO)

Purpose: voting shares decomposition is consistent with the total delegated weight.

```
sum(shares_1, ..., shares_4) = total_note_value
```

**Constructions:** `AddChip`.

## Condition 8: Shares Range (TODO)

Purpose: prevent overflow by ensuring each share fits in the designated bit-width.

```
Each shares_j in [0, 2^24)
```

**Constructions:** `LookupRangeCheckConfig`.

## Condition 9: Shares Hash Integrity (TODO)

Purpose: shares hash is correctly computed over the 4 encrypted shares.

```
shares_hash = H(enc_share_1, enc_share_2, enc_share_3, enc_share_4)
```

**Constructions:** `PoseidonChip`.

## Condition 10: Encryption Integrity (TODO)

Purpose: each ciphertext is a valid El Gamal encryption of the corresponding plaintext share.

```
Each enc_share_i = ElGamal(shares_i, r_i, ea_pk) = (r_i * G, shares_i * G + r_i * ea_pk)
```

**Constructions:** `EccChip`.

## Condition 11: Vote Commitment Integrity (TODO)

Purpose: the public vote commitment is correctly constructed from the shares hash and the vote choice.

```
vote_commitment = H(DOMAIN_VC, shares_hash, proposal_id, vote_decision)
```

**Constructions:** `PoseidonChip`.

## Column Layout

| Columns | Current use | Future use |
|---------|------------|------------|
| `advices[0..5]` | General witness assignment | Sinsemilla/Merkle (if needed) |
| `advices[5]` | Poseidon partial S-box | — |
| `advices[6..9]` | Poseidon state (width 3) | — |
| `advices[9]` | Unused | Range check |
| `lagrange_coeffs[0]` | Constants (DOMAIN_VAN) | Constants (DOMAIN_VC, etc.) |
| `lagrange_coeffs[1]` | Unused | ECC / Sinsemilla |
| `lagrange_coeffs[2..5]` | Poseidon rc_a | — |
| `lagrange_coeffs[5..8]` | Poseidon rc_b | — |
| `primary` | 7 public inputs | — |

CREATE TABLE rounds (
    round_id            TEXT NOT NULL,
    wallet_id           TEXT NOT NULL DEFAULT '',
    network             TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'regtest')),
    snapshot_height     INTEGER NOT NULL,
    ea_pk               BLOB NOT NULL,
    nc_root             BLOB NOT NULL,
    nullifier_imt_root  BLOB NOT NULL,
    session_json        TEXT,
    phase               INTEGER NOT NULL DEFAULT 0,
    created_at          INTEGER NOT NULL,
    bundle_policy_json  TEXT,
    PRIMARY KEY (round_id, wallet_id)
);

CREATE TABLE bundles (
    round_id            TEXT NOT NULL,
    wallet_id           TEXT NOT NULL DEFAULT '',
    bundle_index        INTEGER NOT NULL,
    note_positions_blob BLOB,
    note_identity_hashes_blob BLOB,
    van_comm_rand       BLOB,
    dummy_nullifiers    BLOB,
    rho_signed          BLOB,
    padded_note_data    BLOB,
    nf_signed           BLOB,
    cmx_new             BLOB,
    alpha               BLOB,
    rseed_signed        BLOB,
    rseed_output        BLOB,
    gov_comm            BLOB,
    total_note_value    INTEGER,
    address_index       INTEGER,
    van_leaf_position   INTEGER,
    rk                  BLOB,
    gov_nullifiers_blob BLOB,
    padded_note_secrets BLOB,
    pczt_sighash        BLOB,
    tx1_effects         BLOB,
    delegation_tx_hash  TEXT,
    delegation_pczt     BLOB,
    PRIMARY KEY (round_id, wallet_id, bundle_index),
    FOREIGN KEY (round_id, wallet_id) REFERENCES rounds(round_id, wallet_id) ON DELETE CASCADE
);

CREATE TABLE cached_tree_state (
    round_id        TEXT NOT NULL,
    wallet_id       TEXT NOT NULL DEFAULT '',
    snapshot_height INTEGER NOT NULL,
    tree_state      BLOB NOT NULL,
    PRIMARY KEY (round_id, wallet_id),
    FOREIGN KEY (round_id, wallet_id) REFERENCES rounds(round_id, wallet_id) ON DELETE CASCADE
);

-- `circuit_key_fingerprint` names the delegation circuit key that made
-- `proof`. It is NULL for a proof saved before the fingerprint was recorded;
-- such a proof is sent without one rather than labelled with the key of a
-- later build. The note sits outside the table because SQLite stores comments
-- inside a CREATE statement, and an upgraded database would not have them.
CREATE TABLE proofs (
    round_id      TEXT NOT NULL,
    wallet_id     TEXT NOT NULL DEFAULT '',
    bundle_index  INTEGER NOT NULL,
    witness       BLOB,
    proof         BLOB,
    success       INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    circuit_key_fingerprint BLOB CHECK(circuit_key_fingerprint IS NULL OR length(circuit_key_fingerprint) = 32),
    PRIMARY KEY (round_id, wallet_id, bundle_index),
    FOREIGN KEY (round_id, wallet_id, bundle_index) REFERENCES bundles(round_id, wallet_id, bundle_index) ON DELETE CASCADE
);

CREATE TABLE witnesses (
    round_id        TEXT NOT NULL,
    wallet_id       TEXT NOT NULL DEFAULT '',
    bundle_index    INTEGER NOT NULL,
    note_position   INTEGER NOT NULL,
    note_commitment BLOB NOT NULL,
    root            BLOB NOT NULL,
    auth_path       BLOB NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (round_id, wallet_id, bundle_index, note_position),
    FOREIGN KEY (round_id, wallet_id, bundle_index) REFERENCES bundles(round_id, wallet_id, bundle_index) ON DELETE CASCADE
);

CREATE TABLE votes (
    id              INTEGER PRIMARY KEY,
    round_id        TEXT NOT NULL,
    wallet_id       TEXT NOT NULL DEFAULT '',
    bundle_index    INTEGER NOT NULL,
    proposal_id     INTEGER NOT NULL,
    choice          INTEGER NOT NULL,
    commitment      BLOB,
    created_at      INTEGER NOT NULL,
    tx_hash                 TEXT,
    vc_tree_position        INTEGER,
    commitment_bundle_json  TEXT,
    UNIQUE(round_id, wallet_id, bundle_index, proposal_id),
    FOREIGN KEY (round_id, wallet_id, bundle_index) REFERENCES bundles(round_id, wallet_id, bundle_index) ON DELETE CASCADE
);

CREATE TABLE helper_share_plans (
    round_id                    TEXT NOT NULL,
    wallet_id                   TEXT NOT NULL DEFAULT '',
    bundle_index                INTEGER NOT NULL,
    proposal_id                 INTEGER NOT NULL,
    commitment_bundle_json      TEXT NOT NULL,
    configured_server_urls_json TEXT NOT NULL,
    share_plans_json            TEXT NOT NULL,
    format_version              INTEGER NOT NULL CHECK (format_version = 1),
    placement_guarantee         TEXT NOT NULL CHECK (placement_guarantee IN ('strict','legacy_best_effort')),
    created_at                  INTEGER NOT NULL,
    PRIMARY KEY (round_id, wallet_id, bundle_index, proposal_id),
    FOREIGN KEY (round_id, wallet_id, bundle_index, proposal_id)
        REFERENCES votes(round_id, wallet_id, bundle_index, proposal_id) ON DELETE CASCADE
);

CREATE TRIGGER clear_helper_share_plan_on_vote_generation_change
AFTER UPDATE OF commitment_bundle_json ON votes
WHEN OLD.commitment_bundle_json IS NOT NEW.commitment_bundle_json
BEGIN
    -- Confirmation is the one non-generational recovery update: it fills the
    -- VC tree position in both the vote column and the otherwise-identical
    -- recovery JSON. Advance only a plan bound to the exact OLD snapshot and
    -- only when replacing that one JSON field produces the exact NEW bytes.
    UPDATE helper_share_plans
       SET commitment_bundle_json = NEW.commitment_bundle_json
     WHERE round_id = NEW.round_id AND wallet_id = NEW.wallet_id
       AND bundle_index = NEW.bundle_index AND proposal_id = NEW.proposal_id
       AND commitment_bundle_json = OLD.commitment_bundle_json
       AND OLD.vc_tree_position IS NULL
       AND NEW.vc_tree_position IS NOT NULL
       AND json_set(
               OLD.commitment_bundle_json,
               '$.vc_tree_position',
               NEW.vc_tree_position
           ) = NEW.commitment_bundle_json;
    DELETE FROM helper_share_plans
     WHERE round_id = NEW.round_id AND wallet_id = NEW.wallet_id
       AND bundle_index = NEW.bundle_index AND proposal_id = NEW.proposal_id
       AND commitment_bundle_json IS NOT NEW.commitment_bundle_json;
END;

CREATE TABLE share_delegations (
    round_id        TEXT NOT NULL,
    wallet_id       TEXT NOT NULL DEFAULT '',
    bundle_index    INTEGER NOT NULL,
    proposal_id     INTEGER NOT NULL,
    share_index     INTEGER NOT NULL,
    sent_to_urls    TEXT NOT NULL,
    nullifier       BLOB NOT NULL,
    confirmed       INTEGER NOT NULL DEFAULT 0,
    submit_at       INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    ambiguous_urls  TEXT NOT NULL DEFAULT '[]',
    attempting_urls TEXT NOT NULL DEFAULT '[]',
    target_count    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (round_id, wallet_id, bundle_index, proposal_id, share_index),
    FOREIGN KEY (round_id, wallet_id, bundle_index)
        REFERENCES bundles(round_id, wallet_id, bundle_index) ON DELETE CASCADE
);

CREATE TABLE keystone_signatures (
    round_id        TEXT NOT NULL,
    wallet_id       TEXT NOT NULL DEFAULT '',
    bundle_index    INTEGER NOT NULL,
    sig             BLOB NOT NULL,
    sighash         BLOB NOT NULL,
    rk              BLOB NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (round_id, wallet_id, bundle_index),
    FOREIGN KEY (round_id, wallet_id, bundle_index)
        REFERENCES bundles(round_id, wallet_id, bundle_index) ON DELETE CASCADE
);

CREATE TABLE ballot_intent (
    round_id     TEXT NOT NULL,
    wallet_id    TEXT NOT NULL DEFAULT '',
    proposal_id  INTEGER NOT NULL,
    skipped      INTEGER NOT NULL DEFAULT 0,  -- 1 = intentionally skipped
    choice       INTEGER,                     -- 0-indexed option; NULL iff skipped
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (round_id, wallet_id, proposal_id),
    FOREIGN KEY (round_id, wallet_id) REFERENCES rounds(round_id, wallet_id) ON DELETE CASCADE,
    CHECK ((skipped = 1 AND choice IS NULL) OR (skipped = 0 AND choice IS NOT NULL))
);

CREATE TABLE pir_proof_cache (
    wallet_id   TEXT NOT NULL DEFAULT '',
    network     TEXT NOT NULL CHECK (network IN ('mainnet','testnet','regtest')),
    nullifier   BLOB NOT NULL,
    root        BLOB NOT NULL,
    nf_bounds   BLOB NOT NULL,
    leaf_pos    INTEGER NOT NULL,
    path        BLOB NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (wallet_id, network, root, nullifier)
);

-- Authoritative SDK-owned vote-chain submission lifecycle. One identity per
-- row, created only by runtime reservation; no version-17 evidence is ever
-- imported here. The configured vote-chain id is dispatch routing and is
-- deliberately absent: it binds neither the identity nor the generation
-- digest, except through a proxy batch's `batch_digest`, which is D1 and so
-- binds it by contract. Every row carries the generation digest it was
-- reserved for.
CREATE TABLE chain_submissions (
    identity_key                 BLOB NOT NULL PRIMARY KEY,
    round_id                     TEXT NOT NULL,
    wallet_id                    TEXT NOT NULL DEFAULT '',
    network                      TEXT NOT NULL CHECK (network IN ('mainnet','testnet','regtest')),
    bundle_index                 INTEGER NOT NULL CHECK (bundle_index BETWEEN 0 AND 4294967295),
    kind                         TEXT NOT NULL CHECK (kind IN ('delegation','vote','vote_batch','delegate_and_cast_vote_batch','proxy_delegation_batch')),
    proposal_id                  INTEGER,
    ordered_batch_digest         BLOB,
    generation_digest            BLOB NOT NULL CHECK (length(generation_digest) = 32),
    state                        TEXT NOT NULL CHECK (state IN ('submitting','tracking','recovering','submitted_without_hash','confirmed','rejected')),
    candidate_transaction_hash   BLOB,
    committed_post_reservations  INTEGER NOT NULL DEFAULT 0 CHECK (committed_post_reservations >= 0),
    tracking_started_at          INTEGER,
    diagnostic_kind              TEXT,
    diagnostic                   TEXT,
    confirmation_source          TEXT CHECK (confirmation_source IN ('hash','tree')),
    confirmed_transaction_hash   BLOB,
    final_van_position           INTEGER,
    vote_commitment_positions    BLOB,
    created_at                   INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at                   INTEGER NOT NULL CHECK (updated_at >= created_at),
    FOREIGN KEY (round_id, wallet_id) REFERENCES rounds(round_id, wallet_id) ON DELETE CASCADE,
    CHECK (length(identity_key) >= 32),
    CHECK ((kind = 'delegation' AND proposal_id IS NULL AND ordered_batch_digest IS NULL)
        OR (kind = 'vote' AND proposal_id BETWEEN 1 AND 50 AND ordered_batch_digest IS NULL)
        OR (kind IN ('vote_batch','delegate_and_cast_vote_batch','proxy_delegation_batch') AND proposal_id IS NULL AND length(ordered_batch_digest) = 32)),
    CHECK (candidate_transaction_hash IS NULL OR length(candidate_transaction_hash) = 32),
    CHECK (confirmed_transaction_hash IS NULL OR length(confirmed_transaction_hash) = 32),
    CHECK ((state = 'submitting' AND candidate_transaction_hash IS NULL AND tracking_started_at IS NULL)
        OR (state = 'tracking' AND candidate_transaction_hash IS NOT NULL AND tracking_started_at IS NOT NULL)
        OR state IN ('recovering','submitted_without_hash','confirmed','rejected')),
    CHECK (state != 'submitted_without_hash'
        OR (candidate_transaction_hash IS NULL
            AND confirmed_transaction_hash IS NULL AND final_van_position IS NULL
            AND vote_commitment_positions IS NULL AND diagnostic_kind IS NOT NULL)),
    CHECK ((diagnostic_kind IS NULL) = (diagnostic IS NULL)),
    CHECK (diagnostic IS NULL OR length(CAST(diagnostic AS BLOB)) <= 512),
    CHECK ((state = 'confirmed') = (confirmation_source IS NOT NULL)),
    CHECK (state != 'confirmed'
        OR (final_van_position IS NOT NULL AND vote_commitment_positions IS NOT NULL)),
    CHECK (confirmation_source != 'hash' OR
        (confirmed_transaction_hash IS NOT NULL AND candidate_transaction_hash = confirmed_transaction_hash)),
    CHECK (confirmation_source != 'tree' OR confirmed_transaction_hash IS NULL)
);

CREATE UNIQUE INDEX chain_submissions_identity
    ON chain_submissions(wallet_id, network, round_id, kind, bundle_index,
                         ifnull(proposal_id, -1), ifnull(hex(ordered_batch_digest), ''));
CREATE UNIQUE INDEX chain_submissions_candidate_owner
    ON chain_submissions(candidate_transaction_hash)
    WHERE candidate_transaction_hash IS NOT NULL;
CREATE UNIQUE INDEX chain_submissions_confirmation_hash_owner
    ON chain_submissions(confirmed_transaction_hash)
    WHERE confirmed_transaction_hash IS NOT NULL;

CREATE TRIGGER chain_submissions_immutable_identity
BEFORE UPDATE OF identity_key, round_id, wallet_id, network,
                 bundle_index, kind, proposal_id, ordered_batch_digest,
                 generation_digest, created_at ON chain_submissions
BEGIN
    SELECT RAISE(ABORT, 'chain submission identity and generation are immutable');
END;
CREATE TRIGGER chain_submissions_monotonic_reservations
BEFORE UPDATE OF committed_post_reservations ON chain_submissions
WHEN NEW.committed_post_reservations < OLD.committed_post_reservations
BEGIN
    SELECT RAISE(ABORT, 'chain submission reservation count cannot decrease');
END;
CREATE TRIGGER chain_submissions_immutable_tracking_start
BEFORE UPDATE OF tracking_started_at ON chain_submissions
WHEN OLD.tracking_started_at IS NOT NULL AND NEW.tracking_started_at IS NOT OLD.tracking_started_at
BEGIN
    SELECT RAISE(ABORT, 'chain submission tracking start is immutable');
END;

-- Round-wide immediate helper share designation. One row per wallet and
-- round names the share submitted immediately; it is written once, in the
-- same transaction as the designated vote's helper plan, and never updated.
-- It is voided with the undispatched generation it was made for, on the same
-- condition that clears the vote's helper plan.
CREATE TABLE round_immediate_share (
    round_id      TEXT NOT NULL,
    wallet_id     TEXT NOT NULL DEFAULT '',
    bundle_index  INTEGER NOT NULL CHECK (bundle_index BETWEEN 0 AND 4294967295),
    proposal_id   INTEGER NOT NULL CHECK (proposal_id BETWEEN 1 AND 50),
    share_index   INTEGER NOT NULL CHECK (share_index >= 0),
    designated_at INTEGER NOT NULL CHECK (designated_at >= 0),
    PRIMARY KEY (round_id, wallet_id),
    FOREIGN KEY (round_id, wallet_id, bundle_index, proposal_id)
        REFERENCES votes(round_id, wallet_id, bundle_index, proposal_id) ON DELETE CASCADE
);

CREATE TRIGGER round_immediate_share_immutable
BEFORE UPDATE ON round_immediate_share
BEGIN
    SELECT RAISE(ABORT, 'round immediate share designation is immutable');
END;

CREATE TRIGGER clear_round_immediate_share_on_vote_generation_change
AFTER UPDATE OF commitment_bundle_json ON votes
WHEN OLD.commitment_bundle_json IS NOT NEW.commitment_bundle_json
 AND NOT (
        OLD.vc_tree_position IS NULL
        AND NEW.vc_tree_position IS NOT NULL
        AND json_set(
                OLD.commitment_bundle_json,
                '$.vc_tree_position',
                NEW.vc_tree_position
            ) = NEW.commitment_bundle_json
    )
BEGIN
    DELETE FROM round_immediate_share
     WHERE round_id = NEW.round_id AND wallet_id = NEW.wallet_id
       AND bundle_index = NEW.bundle_index AND proposal_id = NEW.proposal_id;
END;

-- Immutable public delegation authorization; private setup remains in bundles/proofs.
CREATE TABLE delegate_cast_recovery (
    round_id TEXT NOT NULL,
    wallet_id TEXT NOT NULL,
    bundle_index INTEGER NOT NULL,
    batch_digest BLOB NOT NULL CHECK(length(batch_digest) = 32),
    delegation_generation_digest BLOB NOT NULL CHECK(length(delegation_generation_digest) = 32),
    spend_auth_signature BLOB NOT NULL CHECK(length(spend_auth_signature) = 64),
    PRIMARY KEY(round_id, wallet_id, bundle_index, batch_digest),
    FOREIGN KEY(round_id, wallet_id) REFERENCES rounds(round_id, wallet_id) ON DELETE CASCADE
);
CREATE TRIGGER delegate_cast_recovery_immutable BEFORE UPDATE ON delegate_cast_recovery
BEGIN SELECT RAISE(ABORT, 'combined delegation authorization is immutable'); END;

-- Consecutive chain rejections of one bundle's combined delegation-and-cast
-- batch, counted against the delegation generation every recast reuses.
--
-- A terminally rejected combined batch is retired: its members, authorization
-- and lifecycle row are all deleted so the bundle reads `Proved` and can cast
-- afresh. Without this ledger nothing durable survives that retirement, so a
-- rejection caused by the delegation half re-proves every member and re-POSTs
-- the identical delegation on every run, forever.
--
-- Keyed by the delegation generation, not the batch digest: vote commitments
-- are re-randomized on every cast, so an unchanged ballot still produces a new
-- batch digest. The delegation setup is what retirement leaves untouched.
--
-- Deliberately its own table. `chain_submissions` is fingerprinted on every
-- open and rebuilt on drift, and both combined freshness gates admit a new
-- batch only while no row exists there, so the ledger must be invisible to them.
--
-- `last_batch_digest` and the two timestamps are written but never read back
-- by the SDK. They are kept because retirement destroys every other record of
-- what was refused and when, and a host debugging a stuck delegation has
-- nothing else to read; planning deliberately depends only on the streak and
-- the generation.
CREATE TABLE combined_cast_rejections (
    round_id TEXT NOT NULL,
    wallet_id TEXT NOT NULL,
    bundle_index INTEGER NOT NULL,
    delegation_generation_digest BLOB NOT NULL CHECK(length(delegation_generation_digest) = 32),
    consecutive_rejections INTEGER NOT NULL CHECK(consecutive_rejections > 0),
    last_batch_digest BLOB NOT NULL CHECK(length(last_batch_digest) = 32),
    last_diagnostic_kind TEXT NOT NULL,
    last_diagnostic TEXT NOT NULL,
    first_rejected_at INTEGER NOT NULL CHECK(first_rejected_at >= 0),
    last_rejected_at INTEGER NOT NULL CHECK(last_rejected_at >= first_rejected_at),
    PRIMARY KEY(round_id, wallet_id, bundle_index),
    FOREIGN KEY(round_id, wallet_id) REFERENCES rounds(round_id, wallet_id) ON DELETE CASCADE
);
CREATE TRIGGER combined_cast_rejections_monotonic_streak BEFORE UPDATE ON combined_cast_rejections
WHEN NEW.delegation_generation_digest = OLD.delegation_generation_digest
 AND NEW.consecutive_rejections <= OLD.consecutive_rejections
BEGIN SELECT RAISE(ABORT, 'a rejection streak cannot decrease within one delegation generation'); END;
CREATE TRIGGER combined_cast_rejections_generation_restart BEFORE UPDATE ON combined_cast_rejections
WHEN NEW.delegation_generation_digest <> OLD.delegation_generation_digest
 AND NEW.consecutive_rejections <> 1
BEGIN SELECT RAISE(ABORT, 'a new delegation generation restarts the rejection streak'); END;

-- Proxy delegation state (schema version 26). The feature is dormant until
-- `proxy::PROXY_DELEGATION_ENABLED` is set, so these tables stay empty.
--
-- A round's one proxy allocation per wallet: the choices exactly as
-- committed (`[{delegate_index, percent_bps, provider, handle, dk}]`).
-- It is never amended.
CREATE TABLE proxy_allocations (
    round_id        TEXT NOT NULL,
    wallet_id       TEXT NOT NULL DEFAULT '',
    choices_json    TEXT NOT NULL CHECK (json_valid(choices_json)
                                         AND json_type(choices_json) = 'array'
                                         AND json_array_length(choices_json) BETWEEN 1 AND 8),
    committed_at    INTEGER NOT NULL CHECK (committed_at >= 0),
    PRIMARY KEY (round_id, wallet_id),
    FOREIGN KEY (round_id, wallet_id) REFERENCES rounds(round_id, wallet_id) ON DELETE CASCADE
);
CREATE TRIGGER proxy_allocations_immutable BEFORE UPDATE ON proxy_allocations
BEGIN SELECT RAISE(ABORT, 'a committed proxy allocation is immutable'); END;

-- One row per delegation commitment: slot `dc_slot` of one bundle's batch,
-- which is the DC's position in that batch and its circuit slot. The
-- delegate, weight, slot and the batch's share layout are written with the
-- allocation, before any proof, so every re-prove rebuilds the same DC. Batch
-- planning then writes the slot nullifier and the DC's spend-auth randomizer
-- `alpha_v` together, once, before the first proof: re-proving and re-signing
-- reuse them, so the DC's `r_vpk` never changes. The bundle reference does
-- not cascade, so a bundle cannot be pruned from under a committed
-- allocation; deleting the round removes both.
CREATE TABLE proxy_slots (
    round_id       TEXT NOT NULL,
    wallet_id      TEXT NOT NULL DEFAULT '',
    bundle_index   INTEGER NOT NULL CHECK (bundle_index BETWEEN 0 AND 4294967295),
    dc_slot        INTEGER NOT NULL CHECK (dc_slot BETWEEN 0 AND 7),
    delegate_index INTEGER NOT NULL CHECK (delegate_index BETWEEN 1 AND 4294967295),
    weight_ballots INTEGER NOT NULL CHECK (weight_ballots >= 1),
    layout         INTEGER NOT NULL CHECK (layout IN (0, 1)),
    dc_slot_nf     BLOB CHECK (dc_slot_nf IS NULL OR length(dc_slot_nf) = 32),
    alpha_v        BLOB CHECK (alpha_v IS NULL OR length(alpha_v) = 32),
    CHECK ((dc_slot_nf IS NULL) = (alpha_v IS NULL)),
    PRIMARY KEY (round_id, wallet_id, bundle_index, dc_slot),
    UNIQUE (round_id, wallet_id, bundle_index, delegate_index),
    FOREIGN KEY (round_id, wallet_id)
        REFERENCES proxy_allocations(round_id, wallet_id) ON DELETE CASCADE,
    FOREIGN KEY (round_id, wallet_id, bundle_index)
        REFERENCES bundles(round_id, wallet_id, bundle_index)
);
CREATE TRIGGER proxy_slots_immutable_plan
BEFORE UPDATE OF round_id, wallet_id, bundle_index, dc_slot, delegate_index,
                 weight_ballots, layout ON proxy_slots
BEGIN SELECT RAISE(ABORT, 'a planned proxy slot is immutable'); END;
CREATE TRIGGER proxy_slots_batch_plan_written_once
BEFORE UPDATE OF dc_slot_nf, alpha_v ON proxy_slots
WHEN (OLD.dc_slot_nf IS NOT NULL AND NEW.dc_slot_nf IS NOT OLD.dc_slot_nf)
  OR (OLD.alpha_v IS NOT NULL AND NEW.alpha_v IS NOT OLD.alpha_v)
BEGIN SELECT RAISE(ABORT, 'a proxy slot nullifier and randomizer are written once'); END;
CREATE TRIGGER proxy_slots_one_layout_per_batch
BEFORE INSERT ON proxy_slots
WHEN EXISTS (SELECT 1 FROM proxy_slots
              WHERE round_id = NEW.round_id AND wallet_id = NEW.wallet_id
                AND bundle_index = NEW.bundle_index AND layout != NEW.layout)
BEGIN SELECT RAISE(ABORT, 'every slot of a proxy batch shares its layout'); END;

-- A bundle's proved proxy batch: the anchor height its ZKP4 proofs bind,
-- which is 0 exactly when the batch carries the bundle's ZKP1 registration.
-- It is written in one transaction with every DC row below, after batch
-- planning, and never updated; a re-prove against another anchor replaces the
-- whole batch. The bundle reference does not cascade, as for slots.
CREATE TABLE proxy_batches (
    round_id      TEXT NOT NULL,
    wallet_id     TEXT NOT NULL DEFAULT '',
    bundle_index  INTEGER NOT NULL CHECK (bundle_index BETWEEN 0 AND 4294967295),
    anchor_height INTEGER NOT NULL CHECK (anchor_height BETWEEN 0 AND 4294967295),
    proved_at     INTEGER NOT NULL CHECK (proved_at >= 0),
    PRIMARY KEY (round_id, wallet_id, bundle_index),
    FOREIGN KEY (round_id, wallet_id)
        REFERENCES proxy_allocations(round_id, wallet_id) ON DELETE CASCADE,
    FOREIGN KEY (round_id, wallet_id, bundle_index)
        REFERENCES bundles(round_id, wallet_id, bundle_index)
);
CREATE TRIGGER proxy_batches_immutable BEFORE UPDATE ON proxy_batches
BEGIN SELECT RAISE(ABORT, 'a proved proxy batch is immutable'); END;

-- One proved DC of a batch: its ZKP4 proof and the public outputs its 0x09
-- action carries, besides the slot nullifier its slot row holds. Every value
-- here is public once the batch is submitted; the share secrets stay
-- derivable from the round hotkey and are not stored.
CREATE TABLE proxy_batch_dcs (
    round_id                TEXT NOT NULL,
    wallet_id               TEXT NOT NULL DEFAULT '',
    bundle_index            INTEGER NOT NULL CHECK (bundle_index BETWEEN 0 AND 4294967295),
    dc_slot                 INTEGER NOT NULL CHECK (dc_slot BETWEEN 0 AND 7),
    van_nullifier           BLOB NOT NULL CHECK (length(van_nullifier) = 32),
    r_vpk                   BLOB NOT NULL CHECK (length(r_vpk) = 32),
    vote_authority_note_new BLOB NOT NULL CHECK (length(vote_authority_note_new) = 32),
    delegation_commitment   BLOB NOT NULL CHECK (length(delegation_commitment) = 32),
    proof                   BLOB NOT NULL CHECK (length(proof) BETWEEN 1 AND 15360),
    PRIMARY KEY (round_id, wallet_id, bundle_index, dc_slot),
    FOREIGN KEY (round_id, wallet_id, bundle_index)
        REFERENCES proxy_batches(round_id, wallet_id, bundle_index) ON DELETE CASCADE,
    FOREIGN KEY (round_id, wallet_id, bundle_index, dc_slot)
        REFERENCES proxy_slots(round_id, wallet_id, bundle_index, dc_slot) ON DELETE CASCADE
);
CREATE TRIGGER proxy_batch_dcs_immutable BEFORE UPDATE ON proxy_batch_dcs
BEGIN SELECT RAISE(ABORT, 'a proved proxy DC is immutable'); END;

-- A proved batch's signed 0x09 submission, written before the POST of each
-- attempt. `batch_digest` (D1 with expiry 0), the chain id D1 binds and the
-- registration's SpendAuth signature, present exactly when the batch carries
-- the registration, are written once. The newest attempt is its expiry and
-- one 64-byte D1 signature per DC in slot order; a re-sign replaces it only
-- with a later expiry.
CREATE TABLE proxy_batch_attempts (
    round_id                    TEXT NOT NULL,
    wallet_id                   TEXT NOT NULL DEFAULT '',
    bundle_index                INTEGER NOT NULL CHECK (bundle_index BETWEEN 0 AND 4294967295),
    batch_digest                BLOB NOT NULL CHECK (length(batch_digest) = 32),
    chain_id                    TEXT NOT NULL CHECK (length(chain_id) BETWEEN 1 AND 255),
    registration_spend_auth_sig BLOB CHECK (registration_spend_auth_sig IS NULL
                                            OR length(registration_spend_auth_sig) = 64),
    expiry_height               INTEGER NOT NULL CHECK (expiry_height >= 1),
    dc_signatures               BLOB NOT NULL CHECK (length(dc_signatures) BETWEEN 64 AND 512
                                                     AND length(dc_signatures) % 64 = 0),
    signed_at                   INTEGER NOT NULL CHECK (signed_at >= 0),
    PRIMARY KEY (round_id, wallet_id, bundle_index),
    FOREIGN KEY (round_id, wallet_id, bundle_index)
        REFERENCES proxy_batches(round_id, wallet_id, bundle_index) ON DELETE CASCADE
);
CREATE TRIGGER proxy_batch_attempts_identity_immutable
BEFORE UPDATE OF round_id, wallet_id, bundle_index, batch_digest, chain_id,
                 registration_spend_auth_sig ON proxy_batch_attempts
BEGIN SELECT RAISE(ABORT, 'a proxy batch keeps its identity and registration signature'); END;
CREATE TRIGGER proxy_batch_attempts_expiry_rises
BEFORE UPDATE OF expiry_height ON proxy_batch_attempts
WHEN NEW.expiry_height <= OLD.expiry_height
BEGIN SELECT RAISE(ABORT, 'a re-signed proxy batch expires after the attempt it replaces'); END;

-- Where each DC of a landed batch sits in the round's commitment tree,
-- written once by the batch's confirmation together with the bundle's new
-- VAN position. Helper-share delivery reads a DC's tree position here.
CREATE TABLE proxy_dc_landings (
    round_id      TEXT NOT NULL,
    wallet_id     TEXT NOT NULL DEFAULT '',
    bundle_index  INTEGER NOT NULL CHECK (bundle_index BETWEEN 0 AND 4294967295),
    dc_slot       INTEGER NOT NULL CHECK (dc_slot BETWEEN 0 AND 7),
    tree_position INTEGER NOT NULL CHECK (tree_position BETWEEN 0 AND 16777215),
    PRIMARY KEY (round_id, wallet_id, bundle_index, dc_slot),
    FOREIGN KEY (round_id, wallet_id, bundle_index, dc_slot)
        REFERENCES proxy_batch_dcs(round_id, wallet_id, bundle_index, dc_slot) ON DELETE CASCADE
);
CREATE TRIGGER proxy_dc_landings_immutable BEFORE UPDATE ON proxy_dc_landings
BEGIN SELECT RAISE(ABORT, 'a landed proxy DC keeps its tree position'); END;

-- A landed DC's helper-share plan, the DC's counterpart of
-- `helper_share_plans`: the helper fleet it was planned against and one
-- `ShareSubmissionPlan` per share it delivers, written with the DC's share
-- rows before its first share POST and never changed. A single-share DC
-- plans share 0 alone with `submit_at = 0`; a standard DC plans all 16 on the
-- standard schedule. No DC share is ever the round's immediate share.
CREATE TABLE proxy_share_plans (
    round_id                    TEXT NOT NULL,
    wallet_id                   TEXT NOT NULL DEFAULT '',
    bundle_index                INTEGER NOT NULL CHECK (bundle_index BETWEEN 0 AND 4294967295),
    dc_slot                     INTEGER NOT NULL CHECK (dc_slot BETWEEN 0 AND 7),
    configured_server_urls_json TEXT NOT NULL CHECK (json_valid(configured_server_urls_json)
                                                     AND json_type(configured_server_urls_json) = 'array'),
    share_plans_json            TEXT NOT NULL CHECK (json_valid(share_plans_json)
                                                     AND json_type(share_plans_json) = 'array'
                                                     AND json_array_length(share_plans_json) IN (1, 16)),
    created_at                  INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (round_id, wallet_id, bundle_index, dc_slot),
    FOREIGN KEY (round_id, wallet_id, bundle_index, dc_slot)
        REFERENCES proxy_dc_landings(round_id, wallet_id, bundle_index, dc_slot) ON DELETE CASCADE
);
CREATE TRIGGER proxy_share_plans_immutable BEFORE UPDATE ON proxy_share_plans
BEGIN SELECT RAISE(ABORT, 'a DC share plan is immutable'); END;

-- One planned share of a landed DC and its helper delivery state, kept as
-- `share_delegations` keeps a vote share's: canonical helper URLs that
-- definitely accepted it, that may hold it, and that are reserved before a
-- POST, pairwise disjoint. The reveal nullifier, placement target and
-- `submit_at` are fixed by the plan; `confirmed` records the configured
-- helper quorum observing the reveal and is never cleared. `handed_off_at`
-- records when a delivery pass found the share held by its reachable
-- target, `target_count` capped by the configured fleet's size, as a vote
-- share's placement is judged, and is written once. The share's secrets are derived again from the round hotkey for every POST
-- and are not stored.
CREATE TABLE proxy_dc_shares (
    round_id        TEXT NOT NULL,
    wallet_id       TEXT NOT NULL DEFAULT '',
    bundle_index    INTEGER NOT NULL CHECK (bundle_index BETWEEN 0 AND 4294967295),
    dc_slot         INTEGER NOT NULL CHECK (dc_slot BETWEEN 0 AND 7),
    share_index     INTEGER NOT NULL CHECK (share_index BETWEEN 0 AND 15),
    nullifier       BLOB NOT NULL CHECK (length(nullifier) = 32),
    target_count    INTEGER NOT NULL CHECK (target_count >= 1),
    submit_at       INTEGER NOT NULL CHECK (submit_at >= 0),
    sent_to_urls    TEXT NOT NULL DEFAULT '[]',
    ambiguous_urls  TEXT NOT NULL DEFAULT '[]',
    attempting_urls TEXT NOT NULL DEFAULT '[]',
    confirmed       INTEGER NOT NULL DEFAULT 0 CHECK (confirmed IN (0, 1)),
    handed_off_at   INTEGER CHECK (handed_off_at IS NULL OR handed_off_at >= 0),
    created_at      INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (round_id, wallet_id, bundle_index, dc_slot, share_index),
    FOREIGN KEY (round_id, wallet_id, bundle_index, dc_slot)
        REFERENCES proxy_share_plans(round_id, wallet_id, bundle_index, dc_slot) ON DELETE CASCADE
);
CREATE TRIGGER proxy_dc_shares_plan_immutable
BEFORE UPDATE OF round_id, wallet_id, bundle_index, dc_slot, share_index, nullifier,
                 target_count, submit_at, created_at ON proxy_dc_shares
BEGIN SELECT RAISE(ABORT, 'a planned DC share keeps its identity and schedule'); END;
CREATE TRIGGER proxy_dc_shares_confirmation_written_once
BEFORE UPDATE OF confirmed ON proxy_dc_shares
WHEN OLD.confirmed = 1 AND NEW.confirmed != 1
BEGIN SELECT RAISE(ABORT, 'a confirmed DC share stays confirmed'); END;
CREATE TRIGGER proxy_dc_shares_hand_off_written_once
BEFORE UPDATE OF handed_off_at ON proxy_dc_shares
WHEN OLD.handed_off_at IS NOT NULL AND NEW.handed_off_at IS NOT OLD.handed_off_at
BEGIN SELECT RAISE(ABORT, 'a handed off DC share stays handed off'); END;

-- Delegate mode: the one delegate key this account holds on a vote chain
-- (`chain_id`), by its public key. The secret stays in the host's secure
-- storage. A delegate needs no notes, so the row belongs to the account and
-- a vote chain, not a round, and account deletion removes it explicitly.
-- `pending_register_op` is the exact signed register op body, kept so the
-- same bytes are resent until the registration settles; it is cleared when a
-- proven entry shows the key, which writes `delegate_index` once.
CREATE TABLE proxy_delegate_keys (
    wallet_id      TEXT NOT NULL DEFAULT '',
    network        TEXT NOT NULL CHECK (network IN ('mainnet','testnet','regtest')),
    chain_id       TEXT NOT NULL DEFAULT '',
    delegate_key   BLOB NOT NULL CHECK (length(delegate_key) = 32),
    delegate_index INTEGER CHECK (delegate_index IS NULL OR delegate_index BETWEEN 1 AND 4294967295),
    created_at     INTEGER NOT NULL CHECK (created_at >= 0),
    pending_register_op BLOB CHECK (pending_register_op IS NULL
                                    OR length(pending_register_op) BETWEEN 1 AND 16384),
    CHECK (delegate_index IS NULL OR pending_register_op IS NULL),
    PRIMARY KEY (wallet_id, network, chain_id)
);
CREATE TRIGGER proxy_delegate_keys_identity_immutable
BEFORE UPDATE OF wallet_id, network, chain_id, delegate_key, created_at ON proxy_delegate_keys
BEGIN SELECT RAISE(ABORT, 'a delegate key record keeps its identity'); END;
CREATE TRIGGER proxy_delegate_keys_index_written_once
BEFORE UPDATE OF delegate_index ON proxy_delegate_keys
WHEN OLD.delegate_index IS NOT NULL AND NEW.delegate_index IS NOT OLD.delegate_index
BEGIN SELECT RAISE(ABORT, 'a delegate key keeps its registry index'); END;

-- Delegate mode: the delegate's one signed ballot per round, stored before
-- its first submission so every retry resubmits it unchanged. `options` holds
-- one little-endian u32 choice per proposal, proposal 1 first.
CREATE TABLE proxy_delegate_ballots (
    wallet_id      TEXT NOT NULL DEFAULT '',
    network        TEXT NOT NULL CHECK (network IN ('mainnet','testnet','regtest')),
    chain_id       TEXT NOT NULL DEFAULT '',
    round_id       TEXT NOT NULL,
    delegate_index INTEGER NOT NULL CHECK (delegate_index BETWEEN 1 AND 4294967295),
    delegate_key   BLOB NOT NULL CHECK (length(delegate_key) = 32),
    options        BLOB NOT NULL CHECK (length(options) BETWEEN 4 AND 200 AND length(options) % 4 = 0),
    signature      BLOB NOT NULL CHECK (length(signature) = 64),
    signed_at      INTEGER NOT NULL CHECK (signed_at >= 0),
    PRIMARY KEY (wallet_id, network, chain_id, round_id, delegate_index)
);
CREATE TRIGGER proxy_delegate_ballots_immutable BEFORE UPDATE ON proxy_delegate_ballots
BEGIN SELECT RAISE(ABORT, 'a signed delegate ballot is immutable'); END;

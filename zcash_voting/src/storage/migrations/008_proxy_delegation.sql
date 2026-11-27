-- Proxy delegation (schema version 26). The rung first admits the 0x09 kind
-- to `chain_submissions`, rebuilding the table as version 23 did and keeping
-- every row, then creates the proxy tables.
DROP TRIGGER IF EXISTS chain_submissions_immutable_identity;
DROP TRIGGER IF EXISTS chain_submissions_monotonic_reservations;
DROP TRIGGER IF EXISTS chain_submissions_immutable_tracking_start;
DROP INDEX IF EXISTS chain_submissions_identity;
DROP INDEX IF EXISTS chain_submissions_candidate_owner;
DROP INDEX IF EXISTS chain_submissions_confirmation_hash_owner;
ALTER TABLE chain_submissions RENAME TO chain_submissions_v25;

CREATE TABLE chain_submissions (
    identity_key BLOB NOT NULL PRIMARY KEY,
    round_id TEXT NOT NULL,
    wallet_id TEXT NOT NULL DEFAULT '',
    network TEXT NOT NULL CHECK (network IN ('mainnet','testnet','regtest')),
    bundle_index INTEGER NOT NULL CHECK (bundle_index BETWEEN 0 AND 4294967295),
    kind TEXT NOT NULL CHECK (kind IN ('delegation','vote','vote_batch','delegate_and_cast_vote_batch','proxy_delegation_batch')),
    proposal_id INTEGER,
    ordered_batch_digest BLOB,
    generation_digest BLOB NOT NULL CHECK (length(generation_digest) = 32),
    state TEXT NOT NULL CHECK (state IN ('submitting','tracking','recovering','submitted_without_hash','confirmed','rejected')),
    candidate_transaction_hash BLOB,
    committed_post_reservations INTEGER NOT NULL DEFAULT 0 CHECK (committed_post_reservations >= 0),
    tracking_started_at INTEGER,
    diagnostic_kind TEXT,
    diagnostic TEXT,
    confirmation_source TEXT CHECK (confirmation_source IN ('hash','tree')),
    confirmed_transaction_hash BLOB,
    final_van_position INTEGER,
    vote_commitment_positions BLOB,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
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

INSERT INTO chain_submissions SELECT * FROM chain_submissions_v25;
DROP TABLE chain_submissions_v25;

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

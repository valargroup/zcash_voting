CREATE TABLE IF NOT EXISTS ledger_delegation_retries (
    round_id TEXT NOT NULL,
    wallet_id TEXT NOT NULL,
    bundle_index INTEGER NOT NULL,
    PRIMARY KEY (round_id, wallet_id, bundle_index),
    FOREIGN KEY (round_id, wallet_id, bundle_index)
        REFERENCES bundles(round_id, wallet_id, bundle_index) ON DELETE CASCADE
);

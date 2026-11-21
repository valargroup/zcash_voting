use super::*;
use crate::delegation_pipeline::{DelegationPipeline, SqliteWalletDbOpener};
use std::sync::Arc;

/// Keeps the wallet database alive alongside a pipeline with two eligible notes and saved witnesses.
pub(in crate::delegate) fn pipeline_with_wallet(
) -> (DelegationPipeline<SqliteWalletDbOpener>, Connection) {
    let (db, round_params, hotkey, prepared) = prepared_wallet_delegation_fixture();
    let db = Arc::new(db);
    {
        let conn = db.conn();
        let witnesses: Vec<_> = prepared
            .bundle_note_infos
            .iter()
            .map(|note| crate::WitnessData {
                note_commitment: note.commitment.clone(),
                position: note.position,
                auth_path: vec![vec![0; 32]; 32],
                root: round_params.nc_root.clone(),
            })
            .collect();
        crate::storage::queries::store_witnesses(
            &conn,
            &prepared.round_id,
            &db.wallet_id(),
            0,
            &witnesses,
        )
        .unwrap();
    }
    // Keep this connection alive while the pipeline opens independent read handles.
    let path = format!(
        "file:pipeline-{}?mode=memory&cache=shared",
        uuid::Uuid::new_v4()
    );
    let mut conn = Connection::open(&path).unwrap();
    let (account, fvk) = setup_test_account_for_network(&mut conn, Network::Regtest);
    let account_ref = account_internal_id(&conn, &account);
    let height = REGTEST_NU6_3_SNAPSHOT_HEIGHT as u32;
    for (tag, mined_height, multiple, position) in [
        (1, height - 2, 1, 7),
        (2, height - 2, 2, 3),
        (3, height + 4, 3, 11),
    ] {
        insert_ironwood_note(
            &conn,
            account_ref,
            &fvk,
            tag,
            mined_height,
            crate::governance::BALLOT_DIVISOR * multiple,
            position,
        );
    }
    conn.execute("DELETE FROM scan_queue", []).unwrap();
    conn.execute(
        "INSERT INTO scan_queue (block_range_start, block_range_end, priority) VALUES (0, ?1, 10)",
        [height + 1],
    )
    .unwrap();
    for block_height in 0..=height {
        conn.execute("INSERT INTO blocks (height, hash, time, sapling_tree, sapling_commitment_tree_size, orchard_commitment_tree_size, sapling_output_count, orchard_action_count) VALUES (?1, ?2, ?1, ?3, 0, 0, 0, 0)", params![block_height, [block_height as u8; 32], Vec::<u8>::new()]).unwrap();
    }
    let lwd = DelegationLwdInputs {
        network: Network::Regtest,
        round_params,
        resolved_round_name: prepared.round_name.clone(),
        anchor_tree_state_bytes: vec![],
        branch_id_provider: prepared.branch_id_provider.clone(),
    };
    let pipeline = DelegationPipeline::new(
        Arc::clone(&db),
        SqliteWalletDbOpener::new(path, Network::Regtest),
        lwd,
        &account.expose_uuid().to_string(),
        Some(hotkey),
        crate::BundlePolicy::default(),
        None,
    )
    .unwrap();
    (pipeline, conn)
}

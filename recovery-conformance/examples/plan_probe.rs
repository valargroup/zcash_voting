//! What does a sidecar still owe?
//!
//! The plan is this suite's oracle, so when a stage fails for a reason the
//! assertions cannot name, the first question is always what `resume_plan`
//! returned over the durable state that stage left. Reading it from an archived
//! sidecar answers that without provisioning another round.
//!
//! ```text
//! plan_probe <sidecar.db> [account-uuid]
//! ```
//!
//! The round is read from the sidecar rather than passed in, because a
//! conformance sidecar holds exactly one.

fn main() {
    let path = std::env::args().nth(1).expect("usage: plan_probe <sidecar.db> [account]");
    let account = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "8b29d4e6-7940-4570-b2c2-3c7a25ba6922".to_string());

    let connection = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("opening the sidecar");
    let round: String = connection
        .query_row("select round_id from rounds limit 1", [], |row| row.get(0))
        .expect("reading the round id");
    drop(connection);

    let database = zcash_voting::round::VotingDb::open_path(std::path::Path::new(&path))
        .expect("reopening the sidecar");
    database.set_wallet_id(&account);

    let proposals = recovery_conformance::round_run::proposal_ids();
    let plan = zcash_voting::session::resume_plan(&database, &round, &proposals)
        .expect("planning the round");

    println!("round {round}");
    println!("steps ({}):", plan.next_steps.len());
    for (index, step) in plan.next_steps.iter().enumerate() {
        println!("  {index}: {step:?}");
    }
}

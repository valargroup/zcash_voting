//! Offline reader used unchanged against v3.0.0 and the current SDK.
#[path = "../../migration-compat/history.rs"]
mod history;

fn main() -> anyhow::Result<()> {
    history::run()
}

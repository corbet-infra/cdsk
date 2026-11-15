//! Fixtures shared by the integration tests.

use cdrv::{EventLog, Source, Timestamp};

use cdsk::fixture::FixtureState;
use cdsk::layout::{LayoutDecl, SfdiskTable};
use cdsk::{Declaration, Plan};

/// A fixed clock so event ordering in tests is deterministic.
pub fn at() -> Timestamp {
    Timestamp::from_millis(1_760_000_000_000)
}

/// An event log for the synthetic host.
pub fn log() -> EventLog {
    EventLog::new("example-host", Source::Cdsk, 1)
}

/// The recorded host state.
pub fn state() -> FixtureState {
    serde_json::from_str(include_str!("../../fixtures/state.json")).expect("state fixture parses")
}

/// The `nixstorage`-style declaration.
pub fn declaration() -> Declaration {
    serde_json::from_str(include_str!("../../fixtures/declaration.json"))
        .expect("declaration fixture parses")
}

/// The declared disk layout.
pub fn layout() -> LayoutDecl {
    serde_json::from_str(include_str!("../../fixtures/layout.json")).expect("layout fixture parses")
}

/// The live partition table the layout is checked against.
pub fn sfdisk() -> SfdiskTable {
    SfdiskTable::parse(include_str!("../../fixtures/sfdisk.json")).expect("sfdisk fixture parses")
}

/// Every action's argv.
pub fn argvs(plan: &Plan) -> Vec<Vec<String>> {
    plan.actions
        .iter()
        .map(|action| action.argv.clone())
        .collect()
}

/// Whether the plan contains exactly this command.
pub fn has_action(plan: &Plan, argv: &[&str]) -> bool {
    plan.actions.iter().any(|action| action.argv == argv)
}

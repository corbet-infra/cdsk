//! Ownership and top-directory mode convergence.

use cdsk::Plan;
use cdsk::fixture::{FixtureFs, FixtureState, FixtureZfs};

use crate::common;

fn converge(state: &FixtureState) -> cdsk::ConvergePlan {
    cdsk::converge(
        &common::declaration(),
        &FixtureZfs::new(state),
        &FixtureFs::new(state),
        "example-host",
        common::at(),
    )
    .expect("convergence succeeds")
}

fn with<'a>(plan: &'a Plan, program: &str) -> Vec<&'a cdsk::Action> {
    plan.actions
        .iter()
        .filter(|action| action.argv.first().map(String::as_str) == Some(program))
        .collect()
}

#[test]
fn only_mis_owned_paths_are_chowned() {
    let state = common::state();
    let converged = converge(&state);
    let chowns = with(&converged.plan, "chown");

    assert_eq!(chowns.len(), 1, "one recursive chown for the media root");
    assert_eq!(
        chowns[0].argv,
        ["chown", "-h", "1000:100", "/tank/media/archive"]
    );
    assert!(
        !chowns[0]
            .argv
            .iter()
            .any(|arg| arg.contains("/tank/media/scratch")),
        "a pruned subtree is never walked"
    );
    assert!(
        !chowns[0]
            .argv
            .iter()
            .any(|arg| arg.contains("/tank/apps/data/cache")),
        "a carve-out is pruned from the leaf sweep"
    );
    assert!(
        converged
            .report
            .ownership
            .carve_outs
            .contains(&std::path::PathBuf::from("/tank/apps/data/cache"))
    );
}

#[test]
fn ownership_convergence_is_idempotent() {
    let mut state = common::state();
    let archive = state
        .paths
        .get_mut("/tank/media/archive")
        .expect("fixture path");
    archive.uid = 1000;
    archive.gid = 100;
    archive.mode = "0755".to_owned();

    let converged = converge(&state);
    assert!(
        with(&converged.plan, "chown").is_empty(),
        "a converged tree costs no chown"
    );
    assert!(with(&converged.plan, "chmod").is_empty());
    assert!(converged.report.ownership.changes.is_empty());
}

#[test]
fn mode_is_the_top_directory_only() {
    let mut state = common::state();
    state
        .paths
        .get_mut("/tank/media")
        .expect("fixture path")
        .mode = "0755".to_owned();

    let converged = converge(&state);
    assert!(common::has_action(
        &converged.plan,
        &["chmod", "2750", "/tank/media"]
    ));
    assert!(
        with(&converged.plan, "chmod")
            .iter()
            .all(|action| action.argv[2] != "/tank/media/archive"),
        "chmod never recurses"
    );
}

#[test]
fn an_absent_root_is_reported_and_never_created() {
    let mut state = common::state();
    state.paths.remove("/tank/media");

    let converged = converge(&state);
    assert!(
        converged
            .report
            .ownership
            .absent
            .contains(&std::path::PathBuf::from("/tank/media"))
    );
    assert!(
        converged
            .plan
            .actions
            .iter()
            .all(|action| !action.argv.iter().any(|arg| arg == "/tank/media")),
        "an absent path is never created or touched"
    );
    assert!(converged.plan.events.iter().any(|event| {
        event.kind == "dataset.drift" && event.data["reason"] == serde_json::json!("absent")
    }));
}

#[test]
fn leaves_run_shallowest_first() {
    let mut state = common::state();
    state
        .paths
        .get_mut("/tank/apps/data")
        .expect("fixture path")
        .uid = 0;
    state
        .paths
        .get_mut("/tank/apps/data/cache")
        .expect("fixture path")
        .uid = 0;

    // The carve-out is pruned, so only the leaf itself is swept; the test is
    // that its own chown exists and the carve-out's does not.
    let converged = converge(&state);
    let chowns = with(&converged.plan, "chown");
    assert!(
        chowns
            .iter()
            .any(|action| action.argv == ["chown", "-h", "3002:3002", "/tank/apps/data"])
    );
    assert!(
        !chowns
            .iter()
            .any(|action| action.argv.iter().any(|arg| arg.contains("cache")))
    );
}

#[test]
fn a_malformed_mode_fails_the_pass_before_any_action() {
    let mut decl = common::declaration();
    decl.reconciler
        .roots
        .get_mut("/tank/media")
        .expect("fixture root")
        .mode = "9x9".to_owned();
    let state = common::state();

    let error = cdsk::converge(
        &decl,
        &FixtureZfs::new(&state),
        &FixtureFs::new(&state),
        "example-host",
        common::at(),
    )
    .expect_err("a malformed mode must fail");
    assert!(matches!(error, cdsk::Error::Declaration(_)));
}

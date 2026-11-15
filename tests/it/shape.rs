//! Dataset shape convergence.

use cdsk::Plan;
use cdsk::fixture::{FixtureFs, FixtureZfs};

use crate::common;

fn converge(state: &cdsk::fixture::FixtureState) -> cdsk::ConvergePlan {
    cdsk::converge(
        &common::declaration(),
        &FixtureZfs::new(state),
        &FixtureFs::new(state),
        "example-host",
        common::at(),
    )
    .expect("convergence succeeds")
}

fn sets(plan: &Plan) -> Vec<&cdsk::Action> {
    plan.actions
        .iter()
        .filter(|action| action.argv.first().map(String::as_str) == Some("zfs"))
        .collect()
}

#[test]
fn only_the_drifted_properties_are_set() {
    let state = common::state();
    let converged = converge(&state);

    assert!(common::has_action(
        &converged.plan,
        &["zfs", "set", "recordsize=32K", "tank/apps/data"]
    ));
    assert!(
        !sets(&converged.plan)
            .iter()
            .any(|action| action.argv[2].starts_with("compression=")
                && action.argv[3] == "tank/apps/data"),
        "compression already matched and must not be set"
    );
    assert!(
        !sets(&converged.plan).iter().any(
            |action| action.argv[2].starts_with("atime=") && action.argv[3] == "tank/apps/data"
        ),
        "atime already matched locally and must not be set"
    );

    let changes: Vec<_> = converged
        .report
        .shape
        .property_changes
        .iter()
        .filter(|change| change.dataset == "tank/apps/data")
        .collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property, "recordsize");
    assert_eq!(changes[0].current.as_deref(), Some("128K"));
    assert_eq!(changes[0].desired, "32K");

    assert!(converged.plan.events.iter().any(|event| {
        event.kind == "dataset.drift"
            && event.subject == cdrv::Subject::Dataset("tank/apps/data".into())
    }));
}

#[test]
fn a_clean_dataset_reports_converged_and_plans_nothing() {
    let state = common::state();
    let converged = converge(&state);

    assert!(
        !sets(&converged.plan)
            .iter()
            .any(|action| action.argv[3] == "tank/media"),
        "tank/media already matches its class"
    );
    assert!(converged.plan.events.iter().any(|event| {
        event.kind == "dataset.converged"
            && event.subject == cdrv::Subject::Dataset("tank/media".into())
    }));
}

#[test]
fn reconcile_children_applies_the_class_recursively() {
    let state = common::state();
    let converged = converge(&state);

    assert!(common::has_action(
        &converged.plan,
        &["zfs", "set", "recordsize=1M", "tank/media/archive"]
    ));
    assert!(
        !sets(&converged.plan)
            .iter()
            .any(|action| action.argv[2].starts_with("compression=")
                && action.argv[3] == "tank/media/archive")
    );
}

#[test]
fn a_declared_dataset_that_is_absent_is_reported_never_created() {
    let mut state = common::state();
    state.datasets.datasets.remove("tank/apps/data");
    let converged = converge(&state);

    assert_eq!(converged.report.shape.missing, vec!["tank/apps/data"]);
    assert!(
        !converged
            .plan
            .actions
            .iter()
            .any(|action| action.argv.iter().any(|arg| arg.contains("tank/apps/data"))),
        "convergence never creates a dataset"
    );
    assert!(converged.plan.events.iter().any(|event| {
        event.kind == "dataset.drift" && event.data["reason"] == serde_json::json!("missing")
    }));
}

#[test]
fn equal_sizes_in_another_unit_are_not_drift() {
    let mut state = common::state();
    let data = state
        .datasets
        .datasets
        .get_mut("tank/apps/data")
        .expect("fixture dataset");
    let recordsize = data
        .properties
        .get_mut("recordsize")
        .expect("fixture property");
    recordsize.value = "32768".to_owned();
    recordsize.source = Some("local".to_owned());

    let converged = converge(&state);
    assert!(
        converged
            .report
            .shape
            .property_changes
            .iter()
            .all(|change| change.dataset != "tank/apps/data")
    );
}

#[test]
fn an_inherited_property_is_set_so_the_dataset_owns_it() {
    let mut state = common::state();
    let data = state
        .datasets
        .datasets
        .get_mut("tank/apps/data")
        .expect("fixture dataset");
    let compression = data
        .properties
        .get_mut("compression")
        .expect("fixture property");
    compression.source = Some("inherited from tank/apps".to_owned());

    let converged = converge(&state);
    assert!(common::has_action(
        &converged.plan,
        &["zfs", "set", "compression=zstd", "tank/apps/data"]
    ));
}

#[test]
fn an_undeclared_class_is_a_declaration_error() {
    let mut decl = common::declaration();
    let dataset = decl
        .datasets
        .get_mut("tank/apps/data")
        .expect("fixture dataset");
    dataset.class = Some("typo".to_owned());

    let state = common::state();
    let error = cdsk::converge(
        &decl,
        &FixtureZfs::new(&state),
        &FixtureFs::new(&state),
        "example-host",
        common::at(),
    )
    .expect_err("an undeclared class must fail");
    assert!(matches!(error, cdsk::Error::Declaration(_)));
}

//! Save-image rules.

use cdrv::{Severity, Subject};

use cdsk::disks::InstanceDisk;
use cdsk::fixture::FixtureFs;
use cdsk::save::{RestoreOutcome, plan_after_restore, plan_remove_worthless};

use crate::common;

fn example_disk() -> InstanceDisk {
    common::state()
        .instances
        .get("example-vm")
        .cloned()
        .expect("fixture instance")
}

#[test]
fn a_rollback_removes_a_save_image_it_makes_worthless() {
    let state = common::state();
    let fs = FixtureFs::new(&state);
    let action = plan_remove_worthless(&example_disk(), &fs)
        .expect("the fixture has a save image")
        .expect("the save image exists");
    assert_eq!(action.argv, ["rm", "-f", "/var/lib/cdsk/example-vm.save"]);
}

#[test]
fn removing_an_absent_save_image_is_a_no_op() {
    let mut state = common::state();
    state.paths.remove("/var/lib/cdsk/example-vm.save");
    let fs = FixtureFs::new(&state);
    assert!(
        plan_remove_worthless(&example_disk(), &fs)
            .expect("absence is not an error")
            .is_none()
    );
}

#[test]
fn a_failed_restore_keeps_its_image_and_raises_an_event() {
    let state = common::state();
    let fs = FixtureFs::new(&state);
    let plan = plan_after_restore(
        RestoreOutcome::Failed,
        &example_disk(),
        &fs,
        &mut common::log(),
        common::at(),
    )
    .expect("a failed restore is reported");

    assert!(plan.actions.is_empty(), "the image is kept, not removed");
    let event = plan
        .events
        .iter()
        .find(|event| event.kind == "save.failed_restore")
        .expect("a failed restore raises an event");
    assert_eq!(event.severity, Severity::Error);
    assert_eq!(event.subject, Subject::Instance("example-vm".into()));
    assert_eq!(event.data["kept"], serde_json::json!(true));
}

#[test]
fn a_successful_restore_removes_a_leftover_image() {
    let state = common::state();
    let fs = FixtureFs::new(&state);
    let plan = plan_after_restore(
        RestoreOutcome::Succeeded,
        &example_disk(),
        &fs,
        &mut common::log(),
        common::at(),
    )
    .expect("a successful restore is reported");

    assert_eq!(
        plan.actions[0].argv,
        ["rm", "-f", "/var/lib/cdsk/example-vm.save"]
    );
    assert!(plan.events.iter().any(|event| event.kind == "save.removed"));
}

//! Instance disks: 4Kn zvols, clones, snapshots and reset.

use cdrv::{InstanceId, SectorSize};

use cdsk::disks::{
    CLEAN, InstanceDisk, plan_clone_image, plan_create_zvol, plan_reset, plan_snapshot,
};
use cdsk::fixture::{FixtureFs, FixtureZfs, RecordingRunner};
use cdsk::{DiskReset, DiskResetter, Error, Fs, Zfs};

use crate::common;

fn example_disk() -> InstanceDisk {
    common::state()
        .instances
        .get("example-vm")
        .cloned()
        .expect("fixture instance")
}

#[test]
fn a_512_or_512e_zvol_is_refused() {
    let state = common::state();
    let zfs = FixtureZfs::new(&state);
    let fs = FixtureFs::new(&state);
    assert!(zfs.exists("tank/apps/data").unwrap());
    assert!(fs.meta(std::path::Path::new("/absent")).unwrap().is_none());

    for sector in [
        SectorSize {
            logical: 512,
            physical: 512,
        },
        SectorSize {
            logical: 512,
            physical: 4096,
        },
        SectorSize {
            logical: 4096,
            physical: 512,
        },
    ] {
        let mut log = common::log();
        let error = plan_create_zvol(
            "tank/apps/vmdev/example-vm",
            10_240,
            sector,
            &mut log,
            common::at(),
        )
        .expect_err("only 4Kn is created");
        assert!(
            matches!(error, Error::NotFourKn { .. }),
            "{sector:?} must be refused"
        );
    }
}

#[test]
fn a_4kn_zvol_sets_volblocksize_4096() {
    let mut log = common::log();
    let plan = plan_create_zvol(
        "tank/apps/vmdev/example-vm",
        10_240,
        SectorSize::FOUR_KN,
        &mut log,
        common::at(),
    )
    .expect("4Kn is allowed");
    assert_eq!(
        plan.actions[0].argv,
        [
            "zfs",
            "create",
            "-V",
            "10240M",
            "-o",
            "volblocksize=4096",
            "tank/apps/vmdev/example-vm"
        ]
    );
    assert!(plan.events.iter().any(|event| event.kind == "disk.created"));
}

#[test]
fn a_clone_is_followed_by_its_clean_snapshot() {
    let state = common::state();
    let zfs = FixtureZfs::new(&state);
    let mut log = common::log();
    let plan = plan_clone_image(
        "tank/apps/vms/example",
        "tank/apps/vmdev/fresh",
        &zfs,
        &mut log,
        common::at(),
    )
    .expect("the base image has @clean");

    let argvs = common::argvs(&plan);
    assert_eq!(
        argvs[0],
        [
            "zfs",
            "clone",
            "tank/apps/vms/example@clean",
            "tank/apps/vmdev/fresh"
        ]
    );
    assert_eq!(argvs[1], ["zfs", "snapshot", "tank/apps/vmdev/fresh@clean"]);
    assert!(plan.events.iter().any(|event| event.kind == "image.cloned"));
    assert!(
        plan.events
            .iter()
            .any(|event| event.kind == "snapshot.created")
    );
}

#[test]
fn cloning_onto_an_existing_disk_is_refused() {
    let state = common::state();
    let zfs = FixtureZfs::new(&state);
    let mut log = common::log();
    let error = plan_clone_image(
        "tank/apps/vms/example",
        "tank/apps/vmdev/example-vm",
        &zfs,
        &mut log,
        common::at(),
    )
    .expect_err("an instance disk is never reused");
    assert!(matches!(error, Error::Refused(_)));
}

#[test]
fn a_snapshot_is_planned_under_the_declared_name() {
    let mut log = common::log();
    let plan = plan_snapshot(
        "tank/apps/vmdev/example-vm",
        "image-v1",
        &mut log,
        common::at(),
    )
    .expect("a bare snapshot name is valid");
    assert_eq!(
        plan.actions[0].argv,
        ["zfs", "snapshot", "tank/apps/vmdev/example-vm@image-v1"]
    );
}

#[test]
fn reset_rolls_back_and_drops_newer_snapshots_safely() {
    let state = common::state();
    let zfs = FixtureZfs::new(&state);
    let fs = FixtureFs::new(&state);
    let mut log = common::log();

    let plan = plan_reset(&example_disk(), CLEAN, &zfs, &fs, &mut log, common::at())
        .expect("the instance disk is a clone of the declared base");
    let argvs = common::argvs(&plan);
    // The rollback runs first: a failed rollback must not cost the save image.
    assert_eq!(
        argvs[0],
        ["zfs", "rollback", "-r", "tank/apps/vmdev/example-vm@clean"]
    );
    assert_eq!(argvs[1], ["rm", "-f", "/var/lib/cdsk/example-vm.save"]);
    assert_eq!(argvs.len(), 2);
    assert!(plan.events.iter().any(|event| {
        event.kind == "disk.reset" && event.data["removed_save"] == serde_json::json!(true)
    }));
}

#[test]
fn a_reset_never_mentions_a_destroy_or_the_golden_snapshot() {
    let state = common::state();
    let zfs = FixtureZfs::new(&state);
    let fs = FixtureFs::new(&state);
    let mut log = common::log();
    let plan = plan_reset(&example_disk(), CLEAN, &zfs, &fs, &mut log, common::at()).unwrap();

    for action in &plan.actions {
        assert!(!action.argv.iter().any(|arg| arg == "destroy"));
        assert!(
            !action
                .argv
                .iter()
                .any(|arg| arg == "tank/apps/vms/example@clean")
        );
    }
}

#[test]
fn a_base_image_is_never_a_rollback_target() {
    let state = common::state();
    let zfs = FixtureZfs::new(&state);
    let fs = FixtureFs::new(&state);
    let mut log = common::log();

    let base = InstanceDisk {
        instance: InstanceId::from("example-vm"),
        dataset: "tank/apps/vms/example".to_owned(),
        base: "tank/apps/vms/example".to_owned(),
        save_image: None,
    };
    let error = plan_reset(&base, CLEAN, &zfs, &fs, &mut log, common::at())
        .expect_err("the golden image must never roll back");
    assert!(matches!(error, Error::Refused(_)));

    // A dataset that is not a clone (`origin` is `-`) is refused too.
    let not_a_clone = InstanceDisk {
        instance: InstanceId::from("example-vm"),
        dataset: "tank/apps/data".to_owned(),
        base: "tank/apps/vms/example".to_owned(),
        save_image: None,
    };
    let error = plan_reset(&not_a_clone, CLEAN, &zfs, &fs, &mut log, common::at())
        .expect_err("a non-clone must never roll back");
    assert!(matches!(error, Error::Refused(_)));
}

#[test]
fn a_clone_of_a_different_base_is_refused() {
    let state = common::state();
    let zfs = FixtureZfs::new(&state);
    let fs = FixtureFs::new(&state);
    let mut log = common::log();
    let mut disk = example_disk();
    disk.base = "tank/apps/vms/other".to_owned();

    let error = plan_reset(&disk, CLEAN, &zfs, &fs, &mut log, common::at())
        .expect_err("the origin must match the declared base");
    assert!(matches!(error, Error::Refused(_)));
}

#[test]
fn the_disk_reset_seam_executes_the_plan() {
    let state = common::state();
    let zfs = FixtureZfs::new(&state);
    let fs = FixtureFs::new(&state);
    let runner = RecordingRunner::default();
    let resetter = DiskResetter::new(
        &zfs,
        &fs,
        &runner,
        state.instances.values().cloned(),
        "example-host",
        common::at(),
    );

    resetter
        .reset_to_clean(&InstanceId::from("example-vm"))
        .expect("the bound instance resets");
    let actions = runner.actions();
    assert_eq!(
        actions.last().map(|action| action.argv.clone()),
        Some(vec![
            "zfs".to_owned(),
            "rollback".to_owned(),
            "-r".to_owned(),
            "tank/apps/vmdev/example-vm@clean".to_owned()
        ])
    );
    assert!(
        resetter
            .events()
            .iter()
            .any(|event| event.kind == "disk.reset")
    );

    let error = resetter
        .reset_to_clean(&InstanceId::from("unknown"))
        .expect_err("an unbound instance has no disk");
    assert!(error.to_string().contains("no disk binding"));
}

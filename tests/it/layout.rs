//! Layout verification against a declared partition table.

use cdsk::fixture::FixtureTables;
use cdsk::layout::{LayoutDecl, verify};

use crate::common;

fn with_fixture(decl: &LayoutDecl) -> FixtureTables {
    let mut tables = FixtureTables::new();
    for target in decl.targets.values() {
        tables.insert(target.device.clone(), common::sfdisk());
    }
    tables
}

#[test]
fn a_matching_device_passes() {
    let decl = common::layout();
    let tables = with_fixture(&decl);
    let (plan, report) = verify(&decl, &tables, &mut common::log(), common::at()).expect("read");

    assert!(report.ok);
    assert_eq!(report.targets.len(), 1);
    assert!(report.targets[0].ok());
    assert!(plan.actions.is_empty(), "verification never acts");
    assert!(
        plan.events
            .iter()
            .any(|event| event.kind == "layout.verified")
    );
}

#[test]
fn a_wrong_name_and_size_fail() {
    let mut decl = common::layout();
    let target = decl
        .targets
        .get_mut("example-disk")
        .expect("fixture target");
    target.partitions[0].name = "NOT-ESP".to_owned();
    target.partitions[1].size_mib = Some(999);
    let tables = with_fixture(&decl);

    let (plan, report) = verify(&decl, &tables, &mut common::log(), common::at()).expect("read");
    assert!(!report.ok);
    let checks = &report.targets[0].checks;
    assert!(
        checks
            .iter()
            .any(|check| check.field == "name" && !check.ok)
    );
    assert!(
        checks
            .iter()
            .any(|check| check.field == "size" && !check.ok)
    );
    assert!(checks.iter().any(|check| check.field == "type" && check.ok));
    assert!(plan.events.iter().any(|event| event.kind == "layout.drift"));
}

#[test]
fn a_wrong_number_of_partitions_fails_before_per_partition_checks() {
    let mut decl = common::layout();
    decl.targets
        .get_mut("example-disk")
        .expect("fixture target")
        .partitions
        .pop();
    let mut tables = FixtureTables::new();
    tables.insert("/dev/disk/by-id/example-disk", common::sfdisk());

    let (_, report) = verify(&decl, &tables, &mut common::log(), common::at()).expect("read");
    assert!(!report.ok);
    let checks = &report.targets[0].checks;
    assert!(
        checks
            .iter()
            .any(|check| check.field == "count" && !check.ok)
    );
    assert!(!checks.iter().any(|check| check.field == "name"));
}

#[test]
fn a_raw_device_path_is_refused_without_resolving_it() {
    let mut decl = common::layout();
    decl.targets
        .get_mut("example-disk")
        .expect("fixture target")
        .device = "/dev/sdz".into();

    let (_, report) = verify(
        &decl,
        &FixtureTables::new(),
        &mut common::log(),
        common::at(),
    )
    .expect("refusal is reported, not an error");
    assert!(!report.ok);
    assert!(
        report.targets[0]
            .checks
            .iter()
            .any(|check| check.field == "device" && !check.ok)
    );
}

#[test]
fn an_unattached_device_is_skipped() {
    let decl = common::layout();
    let (plan, report) = verify(
        &decl,
        &FixtureTables::new(),
        &mut common::log(),
        common::at(),
    )
    .expect("an absent device is not an error");
    assert!(report.ok);
    assert!(report.targets[0].skipped);
    assert!(plan.events.is_empty());
}

#[test]
fn a_wrong_sector_size_fails() {
    let decl = common::layout();
    let mut tables = FixtureTables::new();
    let mut table = common::sfdisk();
    table.partitiontable.sectorsize = 512;
    tables.insert("/dev/disk/by-id/example-disk", table);

    let (_, report) = verify(&decl, &tables, &mut common::log(), common::at()).expect("read");
    assert!(!report.ok);
    assert!(
        report.targets[0]
            .checks
            .iter()
            .any(|check| check.field == "sectorSize" && !check.ok)
    );
}

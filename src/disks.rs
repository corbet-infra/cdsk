//! Instance disks: 4Kn zvols, image clones, snapshots and reset to clean.
//!
//! Every instance disk is a clone of a base image's `@clean` snapshot. cdsk
//! never destroys a snapshot or a dataset; a reset is `zfs rollback -r` on the
//! instance's own dataset, which is refused unless the dataset is a clone of
//! the declared base. The golden image is therefore never a rollback target.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use cdrv::{
    BlockDevice, Event, EventLog, InstanceId, SectorSize, Severity, Source, Subject, Timestamp,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{Error, Result};
use crate::fs::Fs;
use crate::plan::{Action, Plan, Runner};
use crate::units::parse_size;
use crate::zfs::Zfs;

/// The snapshot name a clean instance boots from.
pub const CLEAN: &str = "clean";

/// The 4Kn block device a zvol named `name` presents.
#[must_use]
pub fn zvol_device(name: &str) -> BlockDevice {
    BlockDevice {
        path: PathBuf::from(format!("/dev/zvol/{name}")),
        serial: None,
        sector: SectorSize::FOUR_KN,
        read_only: false,
    }
}

/// One instance's disk, which a deployment reads from its instance table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceDisk {
    /// The instance the disk belongs to.
    pub instance: InstanceId,
    /// The instance's dataset (a clone of the base image).
    pub dataset: String,
    /// The base image dataset the instance was cloned from.
    pub base: String,
    /// The libvirt managed-save image, when the deployment keeps one.
    #[serde(default)]
    pub save_image: Option<PathBuf>,
}

/// Plan creating a 4Kn zvol.
///
/// Any geometry other than logical 4096 and physical 4096 is refused: 512 and
/// 512e are never a substitute.
pub fn plan_create_zvol(
    name: &str,
    size_mib: u64,
    sector: SectorSize,
    log: &mut EventLog,
    at: Timestamp,
) -> Result<Plan> {
    if !sector.is_four_kn() {
        return Err(Error::NotFourKn {
            what: format!("zvol {name}"),
            logical: sector.logical,
            physical: sector.physical,
        });
    }
    if name.is_empty() || size_mib == 0 {
        return Err(Error::Refused(
            "a zvol needs a name and a positive size".to_owned(),
        ));
    }
    let mut plan = Plan::new();
    plan.push(Action::new([
        "zfs".to_owned(),
        "create".to_owned(),
        "-V".to_owned(),
        format!("{size_mib}M"),
        "-o".to_owned(),
        "volblocksize=4096".to_owned(),
        name.to_owned(),
    ]));
    plan.emit(log.emit(
        at,
        "disk.created",
        Subject::Dataset(name.to_owned()),
        Severity::Info,
        json!({
            "size_mib": size_mib,
            "sector": { "logical": sector.logical, "physical": sector.physical },
        }),
    ));
    Ok(plan)
}

/// Plan cloning a base image's `@clean` snapshot into a new instance disk, and
/// snapshotting the fresh clone.
pub fn plan_clone_image(
    base: &str,
    target: &str,
    zfs: &dyn Zfs,
    log: &mut EventLog,
    at: Timestamp,
) -> Result<Plan> {
    if base == target {
        return Err(Error::Refused(
            "a base image cannot be cloned onto itself".to_owned(),
        ));
    }
    if !zfs.exists(base)? {
        return Err(Error::Refused(format!("base image {base} is not present")));
    }
    if zfs.exists(target)? {
        return Err(Error::Refused(format!(
            "{target} already exists; cdsk never reuses an instance disk"
        )));
    }
    if !zfs.snapshots(base)?.iter().any(|name| name == CLEAN) {
        return Err(Error::Refused(format!(
            "base image {base} has no @{CLEAN} snapshot"
        )));
    }
    let observed = zfs.get(base, &["volblocksize"], false)?;
    let volblocksize = observed
        .get(base)
        .and_then(|properties| properties.get("volblocksize"))
        .map(|value| value.value.as_str())
        .ok_or_else(|| Error::Refused(format!("base image {base} has no observed volblocksize")))?;
    if parse_size(volblocksize) != Some(4096) {
        return Err(Error::Refused(format!(
            "base image {base} has volblocksize {volblocksize}, not 4096; its clones would not be 4Kn"
        )));
    }

    let mut plan = Plan::new();
    plan.push(Action::new([
        "zfs".to_owned(),
        "clone".to_owned(),
        format!("{base}@{CLEAN}"),
        target.to_owned(),
    ]));
    plan.emit(log.emit(
        at,
        "image.cloned",
        Subject::Dataset(target.to_owned()),
        Severity::Info,
        json!({ "base": base, "snapshot": CLEAN }),
    ));
    plan.push(Action::new([
        "zfs".to_owned(),
        "snapshot".to_owned(),
        format!("{target}@{CLEAN}"),
    ]));
    plan.emit(log.emit(
        at,
        "snapshot.created",
        Subject::Dataset(target.to_owned()),
        Severity::Info,
        json!({ "snapshot": CLEAN }),
    ));
    Ok(plan)
}

/// Plan snapshotting an instance disk under a name the class understands.
pub fn plan_snapshot(dataset: &str, name: &str, log: &mut EventLog, at: Timestamp) -> Result<Plan> {
    validate_snapshot_name(name)?;
    let mut plan = Plan::new();
    plan.push(Action::new([
        "zfs".to_owned(),
        "snapshot".to_owned(),
        format!("{dataset}@{name}"),
    ]));
    plan.emit(log.emit(
        at,
        "snapshot.created",
        Subject::Dataset(dataset.to_owned()),
        Severity::Info,
        json!({ "snapshot": name }),
    ));
    Ok(plan)
}

/// Plan rolling an instance disk back to a snapshot, dropping newer snapshots
/// (`rollback -r`) and removing a save image the rollback makes worthless.
///
/// The dataset must be a clone of the declared base image; a dataset that is
/// not a clone is refused, so the base image itself is never rolled back.
pub fn plan_reset(
    disk: &InstanceDisk,
    snapshot: &str,
    zfs: &dyn Zfs,
    fs: &dyn Fs,
    log: &mut EventLog,
    at: Timestamp,
) -> Result<Plan> {
    validate_snapshot_name(snapshot)?;
    if disk.dataset == disk.base {
        return Err(Error::Refused(format!(
            "{} is a base image, not an instance disk",
            disk.dataset
        )));
    }
    if !zfs.exists(&disk.dataset)? {
        return Err(Error::Refused(format!(
            "instance disk {} is not present",
            disk.dataset
        )));
    }
    let observed = zfs.get(&disk.dataset, &["origin"], false)?;
    let origin = observed
        .get(&disk.dataset)
        .and_then(|properties| properties.get("origin"))
        .map(|value| value.value.as_str())
        .unwrap_or("-");
    if origin == "-" {
        return Err(Error::Refused(format!(
            "{} is not a clone (origin is -); rolling it back would roll back a base image",
            disk.dataset
        )));
    }
    let expected = format!("{}@{CLEAN}", disk.base);
    if origin != expected {
        return Err(Error::Refused(format!(
            "{} has origin {origin}, not {expected}",
            disk.dataset
        )));
    }
    if !zfs
        .snapshots(&disk.dataset)?
        .iter()
        .any(|name| name == snapshot)
    {
        return Err(Error::Refused(format!(
            "{} has no @{snapshot} snapshot",
            disk.dataset
        )));
    }

    let mut plan = Plan::new();
    let save_action = crate::save::plan_remove_worthless(disk, fs)?;
    let removed_save = save_action.is_some();
    plan.push(Action::new([
        "zfs".to_owned(),
        "rollback".to_owned(),
        "-r".to_owned(),
        format!("{}@{snapshot}", disk.dataset),
    ]));
    // The save image goes only after the rollback succeeded: a failed rollback
    // leaves the disk as it was, so the image is still worth keeping.
    if let Some(action) = save_action {
        plan.push(action);
    }
    plan.emit(log.emit(
        at,
        "disk.reset",
        Subject::Dataset(disk.dataset.clone()),
        Severity::Info,
        json!({
            "instance": disk.instance,
            "snapshot": snapshot,
            "removed_save": removed_save,
        }),
    ));
    Ok(plan)
}

fn validate_snapshot_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('@') || name.contains('/') {
        return Err(Error::Refused(format!(
            "snapshot name {name:?} is not a bare name"
        )));
    }
    Ok(())
}

/// The storage action `cwld` leaves open, mirrored from `cwld::ports::DiskReset`
/// so that moving the trait into `cdrv` needs no change here.
pub trait DiskReset {
    /// Reset the instance's disk to its clean snapshot.
    fn reset_to_clean(&self, instance: &InstanceId) -> cdrv::Result<()>;
}

/// Implements [`DiskReset`] through [`plan_reset`].
pub struct DiskResetter<'a> {
    zfs: &'a dyn Zfs,
    fs: &'a dyn Fs,
    runner: &'a dyn Runner,
    disks: BTreeMap<InstanceId, InstanceDisk>,
    host: String,
    at: Timestamp,
    events: RefCell<Vec<Event>>,
}

impl<'a> DiskResetter<'a> {
    /// A resetter over the declared instance disks.
    ///
    /// `at` stamps the events of the resets it performs; construct one per
    /// operation, or accept that its events share a timestamp.
    pub fn new(
        zfs: &'a dyn Zfs,
        fs: &'a dyn Fs,
        runner: &'a dyn Runner,
        disks: impl IntoIterator<Item = InstanceDisk>,
        host: impl Into<String>,
        at: Timestamp,
    ) -> Self {
        Self {
            zfs,
            fs,
            runner,
            disks: disks
                .into_iter()
                .map(|disk| (disk.instance.clone(), disk))
                .collect(),
            host: host.into(),
            at,
            events: RefCell::new(Vec::new()),
        }
    }

    /// The events of the resets performed so far.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        self.events.borrow().clone()
    }
}

impl DiskReset for DiskResetter<'_> {
    fn reset_to_clean(&self, instance: &InstanceId) -> cdrv::Result<()> {
        let disk = self
            .disks
            .get(instance)
            .ok_or_else(|| boxed(Error::UnknownInstance(instance.to_string())))?;
        let first_seq = u64::try_from(self.events.borrow().len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut log = EventLog::new(self.host.clone(), Source::Cdsk, first_seq);
        let plan = plan_reset(disk, CLEAN, self.zfs, self.fs, &mut log, self.at).map_err(boxed)?;
        plan.run(self.runner).map_err(boxed)?;
        self.events.borrow_mut().extend(plan.events);
        Ok(())
    }
}

fn boxed(error: Error) -> cdrv::Error {
    Box::new(error)
}

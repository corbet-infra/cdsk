//! Save images: the storage side of the managed-save rules.
//!
//! A managed save is only worth keeping when it was taken from a clean boot
//! and no disk has been rolled back since. Two rules follow: a rollback makes
//! an existing save worthless and removes it, and a failed restore keeps its
//! image (it is the only evidence a human can inspect) and raises an event.

use cdrv::{EventLog, Severity, Subject, Timestamp};
use serde_json::json;

use crate::disks::InstanceDisk;
use crate::error::Result;
use crate::fs::Fs;
use crate::plan::{Action, Plan};

/// How a restore from a managed save ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// The guest resumed.
    Succeeded,
    /// The guest did not resume; the image is kept.
    Failed,
}

/// The `rm -f` that removes a save image a disk rollback has made worthless.
///
/// Returns `None` when the instance keeps no save image or none is present, so
/// the call is idempotent.
pub fn plan_remove_worthless(disk: &InstanceDisk, fs: &dyn Fs) -> Result<Option<Action>> {
    let Some(save) = &disk.save_image else {
        return Ok(None);
    };
    if fs.meta(save)?.is_none() {
        return Ok(None);
    }
    Ok(Some(Action::new([
        "rm".to_owned(),
        "-f".to_owned(),
        save.display().to_string(),
    ])))
}

/// What to do with the save image after a restore attempt.
///
/// A failed restore keeps the image and raises `save.failed_restore`; a
/// successful restore removes a leftover file, because libvirt's restore
/// consumes the image.
pub fn plan_after_restore(
    outcome: RestoreOutcome,
    disk: &InstanceDisk,
    fs: &dyn Fs,
    log: &mut EventLog,
    at: Timestamp,
) -> Result<Plan> {
    let mut plan = Plan::new();
    let Some(save) = &disk.save_image else {
        return Ok(plan);
    };
    let subject = Subject::Instance(disk.instance.clone());

    match outcome {
        RestoreOutcome::Failed => {
            plan.emit(log.emit(
                at,
                "save.failed_restore",
                subject,
                Severity::Error,
                json!({
                    "save_image": save.display().to_string(),
                    "kept": true,
                    "detail": "a failed restore keeps its image for a human to inspect",
                }),
            ));
        }
        RestoreOutcome::Succeeded => {
            if fs.meta(save)?.is_some() {
                plan.push(Action::new([
                    "rm".to_owned(),
                    "-f".to_owned(),
                    save.display().to_string(),
                ]));
                plan.emit(log.emit(
                    at,
                    "save.removed",
                    subject,
                    Severity::Info,
                    json!({
                        "save_image": save.display().to_string(),
                        "reason": "the restore consumed the image",
                    }),
                ));
            }
        }
    }
    Ok(plan)
}

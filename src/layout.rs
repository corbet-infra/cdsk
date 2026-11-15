//! Layout verification: a read-only `sfdisk --json` drift check against a
//! declared partition table, the port of `layout-verify.sh`.
//!
//! This module never writes to a device. It reads the live table, compares
//! partition names, type GUIDs and sizes, and reports; fixing drift means
//! re-running the image-write path by hand, outside cdsk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cdrv::{EventLog, Severity, Subject, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{Error, Result};
use crate::plan::{Plan, capture};

/// A declared disk layout (`nixstorage.layout`), as JSON.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LayoutDecl {
    /// How far a partition size may be from the declaration before it is drift.
    #[serde(default = "default_tolerance", rename = "toleranceMiB")]
    pub tolerance_mib: u64,
    /// Targets, by name.
    #[serde(default)]
    pub targets: BTreeMap<String, LayoutTarget>,
}

fn default_tolerance() -> u64 {
    1
}

/// One declared device and the partition table it must carry.
#[derive(Debug, Clone, Deserialize)]
pub struct LayoutTarget {
    /// The live device; always a stable `/dev/disk/by-*` path.
    pub device: PathBuf,
    /// The sector size the layout was built for.
    #[serde(default = "four_kn_sector", rename = "sectorSize")]
    pub sector_size: u32,
    /// The partitions, in on-disk order.
    #[serde(default)]
    pub partitions: Vec<LayoutPartition>,
}

fn four_kn_sector() -> u32 {
    4096
}

/// One declared partition.
#[derive(Debug, Clone, Deserialize)]
pub struct LayoutPartition {
    /// The GPT partition name.
    pub name: String,
    /// The GPT type GUID; compared case-insensitively.
    #[serde(rename = "typeGuid")]
    pub type_guid: String,
    /// The size in MiB, or `null` for the last partition that consumes the
    /// remainder (its size is then not asserted).
    #[serde(default, rename = "sizeMiB")]
    pub size_mib: Option<u64>,
}

/// An `sfdisk --json` document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SfdiskTable {
    /// The table itself.
    #[serde(default)]
    pub partitiontable: SfdiskPartitionTable,
}

/// The `partitiontable` object of an `sfdisk --json` document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SfdiskPartitionTable {
    /// The sector size `sfdisk` reports for the device.
    #[serde(default)]
    pub sectorsize: u32,
    /// The partitions, in on-disk order.
    #[serde(default)]
    pub partitions: Vec<SfdiskPartition>,
}

/// One partition in an `sfdisk --json` document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SfdiskPartition {
    /// The GPT partition name.
    #[serde(default)]
    pub name: Option<String>,
    /// The GPT type GUID.
    #[serde(default, rename = "type")]
    pub type_guid: Option<String>,
    /// The size in sectors.
    #[serde(default)]
    pub size: u64,
}

impl SfdiskTable {
    /// Parse an `sfdisk --json` document.
    pub fn parse(text: &str) -> Result<Self> {
        serde_json::from_str(text).map_err(|error| Error::json("sfdisk --json", error))
    }

    /// The sector size, with `sfdisk`'s 512-byte default when it is absent.
    #[must_use]
    pub fn sector_size(&self) -> u32 {
        if self.partitiontable.sectorsize == 0 {
            512
        } else {
            self.partitiontable.sectorsize
        }
    }
}

/// What reading a device's partition table produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableRead {
    /// The device is attached and its table was read.
    Table(SfdiskTable),
    /// The device is not attached right now.
    Absent,
}

/// Partition tables of live devices, behind a trait so tests use fixtures.
pub trait PartitionTables {
    /// Read the table of `device`.
    fn read(&self, device: &Path) -> Result<TableRead>;
}

/// The real thing: `sfdisk --json`, a listing command that never writes.
#[derive(Debug, Clone)]
pub struct CommandSfdisk {
    program: String,
}

impl CommandSfdisk {
    /// The `sfdisk` on `PATH`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            program: "sfdisk".to_owned(),
        }
    }

    /// An `sfdisk` at another path.
    #[must_use]
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

impl Default for CommandSfdisk {
    fn default() -> Self {
        Self::new()
    }
}

impl PartitionTables for CommandSfdisk {
    fn read(&self, device: &Path) -> Result<TableRead> {
        let resolved = match std::fs::canonicalize(device) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TableRead::Absent);
            }
            Err(error) => return Err(Error::io(device, error)),
        };
        let argv = vec![
            self.program.clone(),
            "--json".to_owned(),
            resolved.display().to_string(),
        ];
        let text = capture(&argv)?;
        Ok(TableRead::Table(SfdiskTable::parse(&text)?))
    }
}

/// One field's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// The 1-based partition number, or `None` for a whole-table check.
    pub partition: Option<usize>,
    /// The field checked (`device`, `sectorSize`, `count`, `name`, `type`,
    /// `size`).
    pub field: &'static str,
    /// Whether it matched.
    pub ok: bool,
    /// What was found.
    pub detail: String,
}

/// One target's result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetReport {
    /// The target name.
    pub name: String,
    /// Whether the device was not attached and the target was skipped.
    pub skipped: bool,
    /// Every check performed.
    pub checks: Vec<Check>,
}

impl TargetReport {
    /// Whether every check passed.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|check| check.ok)
    }
}

/// The whole verification's result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutReport {
    /// One entry per declared target.
    pub targets: Vec<TargetReport>,
    /// Whether every attached target matched its declaration.
    pub ok: bool,
}

/// Verify every declared target against its live device.
pub fn verify(
    decl: &LayoutDecl,
    tables: &dyn PartitionTables,
    log: &mut EventLog,
    at: Timestamp,
) -> Result<(Plan, LayoutReport)> {
    let mut plan = Plan::new();
    let mut report = LayoutReport {
        ok: true,
        targets: Vec::new(),
    };

    if decl.targets.is_empty() {
        plan.emit(log.emit(
            at,
            "layout.verified",
            Subject::Host,
            Severity::Info,
            json!({ "skipped": "no layout targets declared" }),
        ));
        return Ok((plan, report));
    }

    for (name, target) in &decl.targets {
        let mut target_report = TargetReport {
            name: name.clone(),
            ..TargetReport::default()
        };

        if !target.device.to_string_lossy().starts_with("/dev/disk/by-") {
            target_report.checks.push(Check {
                partition: None,
                field: "device",
                ok: false,
                detail: format!(
                    "{} is not a /dev/disk/by-* path; refusing to resolve it",
                    target.device.display()
                ),
            });
        } else {
            match tables.read(&target.device)? {
                TableRead::Absent => target_report.skipped = true,
                TableRead::Table(table) => {
                    verify_table(target, &table, decl.tolerance_mib, &mut target_report);
                }
            }
        }

        if target_report.skipped {
            report.targets.push(target_report);
            continue;
        }
        let ok = target_report.ok();
        report.ok &= ok;
        let subject = Subject::Dataset(name.clone());
        if ok {
            plan.emit(log.emit(
                at,
                "layout.verified",
                subject,
                Severity::Info,
                json!({ "device": target.device.display().to_string() }),
            ));
        } else {
            let failed: Vec<&Check> = target_report
                .checks
                .iter()
                .filter(|check| !check.ok)
                .collect();
            plan.emit(log.emit(
                at,
                "layout.drift",
                subject,
                Severity::Error,
                json!({
                    "device": target.device.display().to_string(),
                    "failed": failed,
                    "detail": "this check only reads; fixing drift means re-running the image-write path by hand",
                }),
            ));
        }
        report.targets.push(target_report);
    }

    Ok((plan, report))
}

fn verify_table(
    target: &LayoutTarget,
    table: &SfdiskTable,
    tolerance_mib: u64,
    report: &mut TargetReport,
) {
    let sector_size = table.sector_size();
    report.checks.push(Check {
        partition: None,
        field: "sectorSize",
        ok: sector_size == target.sector_size,
        detail: format!(
            "declared {} bytes, device reports {sector_size}",
            target.sector_size
        ),
    });

    let actual = &table.partitiontable.partitions;
    if actual.len() != target.partitions.len() {
        report.checks.push(Check {
            partition: None,
            field: "count",
            ok: false,
            detail: format!(
                "declared {} partition(s), device has {}",
                target.partitions.len(),
                actual.len()
            ),
        });
        return;
    }
    report.checks.push(Check {
        partition: None,
        field: "count",
        ok: true,
        detail: format!("{} partition(s)", actual.len()),
    });

    for (index, (expected, found)) in target.partitions.iter().zip(actual).enumerate() {
        let partition = Some(index + 1);

        let found_name = found.name.clone().unwrap_or_default();
        report.checks.push(Check {
            partition,
            field: "name",
            ok: found_name == expected.name,
            detail: format!("declared {:?}, device has {:?}", expected.name, found_name),
        });

        let expected_guid = expected.type_guid.to_ascii_lowercase();
        let found_guid = found.type_guid.clone().unwrap_or_default();
        report.checks.push(Check {
            partition,
            field: "type",
            ok: found_guid.to_ascii_lowercase() == expected_guid,
            detail: format!("declared {}, device has {found_guid}", expected.type_guid),
        });

        if let Some(size_mib) = expected.size_mib {
            let found_mib = found.size.saturating_mul(u64::from(sector_size)) / (1024 * 1024);
            let difference = found_mib.abs_diff(size_mib);
            report.checks.push(Check {
                partition,
                field: "size",
                ok: difference <= tolerance_mib,
                detail: format!(
                    "declared {size_mib} MiB, device reports {found_mib} MiB (diff {difference} MiB, tolerance {tolerance_mib} MiB)"
                ),
            });
        }
    }
}

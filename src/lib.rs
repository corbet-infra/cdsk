//! `cdsk` — the storage software behind `nixstorage`'s declarations.
//!
//! It does four things, all through declared data:
//!
//! - converges a dataset's **shape** (`recordsize`, `compression`, any other
//!   property) by planning only the `zfs set` calls that close observed drift;
//!   it never creates or destroys a dataset,
//! - converges **ownership and top-directory mode** on declared roots and
//!   leaves, the port of `nixstorage`'s `reconcile.sh`,
//! - manages **instance disks**: 4Kn zvols, clones of a base image's `@clean`
//!   snapshot, snapshots, and reset to `@clean` by rollback,
//! - verifies a live **partition layout** with `sfdisk --json`, read-only.
//!
//! ZFS is read through the `zfs` CLI's JSON output (OpenZFS 2.3+) behind the
//! [`Zfs`] trait; the filesystem behind [`Fs`]; partition tables behind
//! [`layout::PartitionTables`]. Every change is an [`Action`] in a [`Plan`],
//! so planning executes nothing. A host's real data stays out of the crate:
//! fixtures and tests use synthetic names only.
//!
//! ```
//! use cdrv::{EventLog, SectorSize, Source, Timestamp};
//! use cdsk::disks::{plan_create_zvol, zvol_device};
//!
//! let device = zvol_device("tank/apps/vmdev/example-vm");
//! assert!(device.sector.is_four_kn());
//!
//! let mut log = EventLog::new("example-host", Source::Cdsk, 1);
//! let plan = plan_create_zvol(
//!     "tank/apps/vmdev/example-vm",
//!     10_240,
//!     SectorSize::FOUR_KN,
//!     &mut log,
//!     Timestamp::from_millis(0),
//! )?;
//! assert_eq!(plan.actions.len(), 1);
//!
//! let refused = plan_create_zvol(
//!     "tank/apps/vmdev/example-vm",
//!     10_240,
//!     SectorSize { logical: 512, physical: 512 },
//!     &mut log,
//!     Timestamp::from_millis(0),
//! );
//! assert!(matches!(refused, Err(cdsk::Error::NotFourKn { .. })));
//! # Ok::<(), cdsk::Error>(())
//! ```
#![forbid(unsafe_code)]

pub mod decl;
pub mod disks;
pub mod error;
pub mod fixture;
pub mod fs;
pub mod layout;
pub mod owner;
pub mod plan;
pub mod save;
pub mod shape;
pub mod units;
pub mod zfs;

pub use decl::Declaration;
pub use disks::{DiskReset, DiskResetter, InstanceDisk};
pub use error::{Error, Result};
pub use fs::{Fs, RealFs};
pub use layout::LayoutDecl;
pub use plan::{Action, CommandRunner, Plan, Runner};
pub use zfs::{CommandZfs, Zfs};

use cdrv::{EventLog, Source, Timestamp};

/// Everything one convergence pass found.
#[derive(Debug, Clone, Default)]
pub struct ConvergeReport {
    /// Dataset shape findings.
    pub shape: shape::ShapeReport,
    /// Ownership and mode findings.
    pub ownership: owner::OwnershipReport,
}

/// One convergence pass: the actions it would run and the events it accounts
/// for.
#[derive(Debug, Clone)]
pub struct ConvergePlan {
    /// The planned actions.
    pub plan: Plan,
    /// What convergence found.
    pub report: ConvergeReport,
}

/// Converge dataset shape, then ownership and mode, from one declaration.
///
/// Planning is pure: use [`Plan::run`] with a runner (the CLI does so only
/// with `--apply`) to execute it.
pub fn converge(
    decl: &Declaration,
    zfs: &dyn Zfs,
    fs: &dyn Fs,
    host: &str,
    at: Timestamp,
) -> Result<ConvergePlan> {
    let mut log = EventLog::new(host, Source::Cdsk, 1);
    let (mut plan, shape) = shape::converge(decl, zfs, &mut log, at)?;
    let (ownership_plan, ownership) = owner::converge(decl, fs, &mut log, at)?;
    plan.absorb(ownership_plan);
    Ok(ConvergePlan {
        plan,
        report: ConvergeReport { shape, ownership },
    })
}

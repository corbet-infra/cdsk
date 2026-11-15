//! Ownership and top-directory mode convergence, the port of `reconcile.sh`.
//!
//! Order is the contract: roots first (a root's recursive sweep may reclaim a
//! nested app leaf, which must therefore run later), then leaves shallowest
//! first so the most specific declaration wins. `chown` is mis-owned-only and
//! always `-h`; `chmod` is the top directory only, never `-R`.

use std::path::{Path, PathBuf};

use cdrv::{EventLog, Severity, Subject, Timestamp};
use serde_json::json;

use crate::decl::{Declaration, Owned};
use crate::error::Result;
use crate::fs::Fs;
use crate::plan::{Action, Plan};

/// What ownership convergence found and planned.
#[derive(Debug, Clone, Default)]
pub struct OwnershipReport {
    /// Every ownership or mode change.
    pub changes: Vec<OwnershipChange>,
    /// Declared paths that are absent; they are never created.
    pub absent: Vec<PathBuf>,
    /// Paths declared `reconcile = false`, reported and never touched.
    pub carve_outs: Vec<PathBuf>,
}

/// One ownership or mode change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipChange {
    /// The declared root or leaf.
    pub path: PathBuf,
    /// `owner` or `mode`.
    pub field: &'static str,
    /// What the path has now.
    pub current: String,
    /// What the declaration wants.
    pub desired: String,
    /// How many paths the change covers (one for a top-directory change).
    pub paths: usize,
}

/// Which declaration a path came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Root,
    Leaf,
}

/// Plan the ownership half of a convergence pass.
pub fn converge(
    decl: &Declaration,
    fs: &dyn Fs,
    log: &mut EventLog,
    at: Timestamp,
) -> Result<(Plan, OwnershipReport)> {
    // A malformed mode fails the whole pass before any action is planned.
    for owned in decl
        .reconciler
        .roots
        .values()
        .chain(decl.reconciler.leaves.values())
    {
        owned.mode_bits()?;
    }

    let mut prune: Vec<PathBuf> = decl.reconciler.prune.iter().map(PathBuf::from).collect();
    for (path, owned) in decl
        .reconciler
        .roots
        .iter()
        .chain(decl.reconciler.leaves.iter())
    {
        if !owned.reconcile {
            prune.push(PathBuf::from(path));
        }
    }

    let mut plan = Plan::new();
    let mut report = OwnershipReport::default();

    for (path, owned) in &decl.reconciler.roots {
        reconcile_one(
            Path::new(path),
            owned,
            Kind::Root,
            &prune,
            fs,
            log,
            at,
            &mut plan,
            &mut report,
        )?;
    }

    let mut leaves: Vec<(&String, &Owned)> = decl.reconciler.leaves.iter().collect();
    leaves.sort_by(|(left, _), (right, _)| {
        left.len().cmp(&right.len()).then_with(|| left.cmp(right))
    });
    for (path, owned) in leaves {
        reconcile_one(
            Path::new(path),
            owned,
            Kind::Leaf,
            &prune,
            fs,
            log,
            at,
            &mut plan,
            &mut report,
        )?;
    }

    Ok((plan, report))
}

#[allow(clippy::too_many_arguments)]
fn reconcile_one(
    path: &Path,
    owned: &Owned,
    kind: Kind,
    prune: &[PathBuf],
    fs: &dyn Fs,
    log: &mut EventLog,
    at: Timestamp,
    plan: &mut Plan,
    report: &mut OwnershipReport,
) -> Result<()> {
    let shown = path.display().to_string();
    let subject = || Subject::Dataset(shown.clone());

    if !owned.reconcile {
        report.carve_outs.push(path.to_path_buf());
        plan.emit(log.emit(
            at,
            "dataset.converged",
            subject(),
            Severity::Info,
            json!({ "path": shown, "carve_out": true, "detail": "declared, never touched" }),
        ));
        return Ok(());
    }

    let Some(metadata) = fs.meta(path)? else {
        report.absent.push(path.to_path_buf());
        plan.emit(log.emit(
            at,
            "dataset.drift",
            subject(),
            Severity::Warn,
            json!({
                "path": shown,
                "reason": "absent",
                "detail": "absent paths are reported, never created",
            }),
        ));
        return Ok(());
    };
    if !metadata.is_dir {
        report.absent.push(path.to_path_buf());
        plan.emit(log.emit(
            at,
            "dataset.drift",
            subject(),
            Severity::Warn,
            json!({
                "path": shown,
                "reason": "not_a_directory",
                "detail": "only directories are reconciled",
            }),
        ));
        return Ok(());
    }

    let mode = owned.mode_bits()?;
    let recursive = kind == Kind::Leaf || owned.recurse;
    let wanted = format!("{}:{}", owned.uid, owned.gid);
    let mut changed = false;

    if recursive {
        let paths = fs.mis_owned(path, owned.uid, owned.gid, prune)?;
        if !paths.is_empty() {
            let mut argv = vec!["chown".to_owned(), "-h".to_owned(), wanted.clone()];
            argv.extend(
                paths
                    .iter()
                    .map(|candidate| candidate.display().to_string()),
            );
            plan.push(Action::new(argv));
            report.changes.push(OwnershipChange {
                path: path.to_path_buf(),
                field: "owner",
                current: "mis-owned paths".to_owned(),
                desired: wanted.clone(),
                paths: paths.len(),
            });
            plan.emit(log.emit(
                at,
                "dataset.drift",
                subject(),
                Severity::Warn,
                json!({
                    "path": shown,
                    "field": "owner",
                    "desired": wanted,
                    "recursive": true,
                    "paths": paths.len(),
                }),
            ));
            changed = true;
        }
    } else if (metadata.uid, metadata.gid) != (owned.uid, owned.gid) {
        plan.push(Action::new([
            "chown".to_owned(),
            "-h".to_owned(),
            wanted.clone(),
            shown.clone(),
        ]));
        report.changes.push(OwnershipChange {
            path: path.to_path_buf(),
            field: "owner",
            current: format!("{}:{}", metadata.uid, metadata.gid),
            desired: wanted.clone(),
            paths: 1,
        });
        plan.emit(log.emit(
            at,
            "dataset.drift",
            subject(),
            Severity::Warn,
            json!({
                "path": shown,
                "field": "owner",
                "current": format!("{}:{}", metadata.uid, metadata.gid),
                "desired": wanted,
            }),
        ));
        changed = true;
    }

    if metadata.permissions() != mode {
        let desired_mode = format!("{mode:04o}");
        plan.push(Action::new([
            "chmod".to_owned(),
            desired_mode.clone(),
            shown.clone(),
        ]));
        report.changes.push(OwnershipChange {
            path: path.to_path_buf(),
            field: "mode",
            current: format!("{:04o}", metadata.permissions()),
            desired: desired_mode.clone(),
            paths: 1,
        });
        plan.emit(log.emit(
            at,
            "dataset.drift",
            subject(),
            Severity::Warn,
            json!({
                "path": shown,
                "field": "mode",
                "current": format!("{:04o}", metadata.permissions()),
                "desired": desired_mode,
                "detail": "top directory only",
            }),
        ));
        changed = true;
    }

    if !changed {
        plan.emit(log.emit(
            at,
            "dataset.converged",
            subject(),
            Severity::Info,
            json!({ "path": shown }),
        ));
    }
    Ok(())
}

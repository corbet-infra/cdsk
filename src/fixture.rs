//! Fixture-backed observations: the `--state` file, and recording doubles for
//! tests.
//!
//! A live cdsk reads ZFS and the filesystem through [`crate::zfs::Zfs`] and
//! [`crate::fs::Fs`]. The CLI's `--state` file carries the same facts recorded
//! from a host (raw `zfs get -j` / `zfs list -j` documents, `lstat`-like path
//! entries and the instance disk bindings), so a plan can be inspected without
//! touching a pool.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::decl::parse_mode;
use crate::disks::InstanceDisk;
use crate::error::Result;
use crate::fs::{FileMeta, Fs};
use crate::layout::{PartitionTables, SfdiskTable, TableRead};
use crate::plan::{Action, Runner};
use crate::zfs::{PropertyMap, PropertySets, Zfs, ZfsJson};

/// Recorded observations that stand in for a live host.
///
/// Pool and dataset names in a fixture are synthetic; a deployment's real
/// state never lives in the repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureState {
    /// The `zfs get -j -r` document.
    #[serde(default)]
    pub datasets: ZfsJson,
    /// The `zfs list -j -r -t snapshot` document.
    #[serde(default)]
    pub snapshots: ZfsJson,
    /// Paths as `lstat` would report them, keyed by absolute path.
    #[serde(default)]
    pub paths: BTreeMap<String, FixtureMeta>,
    /// Instance disk bindings, which a deployment reads from its instance
    /// table.
    #[serde(default)]
    pub instances: BTreeMap<String, InstanceDisk>,
}

/// One recorded path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureMeta {
    /// The owning uid.
    pub uid: u32,
    /// The owning gid.
    pub gid: u32,
    /// Octal mode text, for example `"2750"`.
    pub mode: String,
    /// Whether the path is a directory.
    #[serde(default)]
    pub dir: bool,
}

/// [`Zfs`] over recorded documents.
pub struct FixtureZfs<'a> {
    state: &'a FixtureState,
}

impl<'a> FixtureZfs<'a> {
    /// Observe `state`.
    #[must_use]
    pub fn new(state: &'a FixtureState) -> Self {
        Self { state }
    }
}

impl Zfs for FixtureZfs<'_> {
    fn get(&self, target: &str, properties: &[&str], recursive: bool) -> Result<PropertySets> {
        let prefix = format!("{target}/");
        let mut found = PropertySets::new();
        for (name, dataset) in &self.state.datasets.datasets {
            if name != target && !(recursive && name.starts_with(&prefix)) {
                continue;
            }
            let mut map = PropertyMap::new();
            for property in properties {
                if let Some(value) = dataset.properties.get(*property) {
                    map.insert((*property).to_owned(), value.clone());
                }
            }
            found.insert(name.clone(), map);
        }
        Ok(found)
    }

    fn exists(&self, dataset: &str) -> Result<bool> {
        Ok(self.state.datasets.datasets.contains_key(dataset))
    }

    fn datasets(&self, root: &str) -> Result<Vec<String>> {
        let prefix = format!("{root}/");
        let mut found: Vec<String> = self
            .state
            .datasets
            .datasets
            .keys()
            .filter(|name| *name == root || name.starts_with(&prefix))
            .cloned()
            .collect();
        found.sort();
        Ok(found)
    }

    fn snapshots(&self, dataset: &str) -> Result<Vec<String>> {
        let prefix = format!("{dataset}@");
        let mut found: Vec<String> = self
            .state
            .snapshots
            .datasets
            .keys()
            .filter_map(|name| name.strip_prefix(&prefix).map(str::to_owned))
            .collect();
        found.sort();
        found.dedup();
        Ok(found)
    }
}

/// [`Fs`] over recorded path metadata.
pub struct FixtureFs<'a> {
    state: &'a FixtureState,
}

impl<'a> FixtureFs<'a> {
    /// Observe `state`.
    #[must_use]
    pub fn new(state: &'a FixtureState) -> Self {
        Self { state }
    }
}

impl Fs for FixtureFs<'_> {
    fn meta(&self, path: &Path) -> Result<Option<FileMeta>> {
        let Some(recorded) = self.state.paths.get(&path.display().to_string()) else {
            return Ok(None);
        };
        Ok(Some(FileMeta {
            uid: recorded.uid,
            gid: recorded.gid,
            mode: parse_mode(&recorded.mode)?,
            is_dir: recorded.dir,
        }))
    }

    fn mis_owned(
        &self,
        root: &Path,
        uid: u32,
        gid: u32,
        prune: &[PathBuf],
    ) -> Result<Vec<PathBuf>> {
        let root_text = root.display().to_string();
        let mut found = Vec::new();
        for (path, recorded) in &self.state.paths {
            let candidate = PathBuf::from(path);
            if path != &root_text && !candidate.starts_with(root) {
                continue;
            }
            if path != &root_text && prune.iter().any(|pruned| candidate.starts_with(pruned)) {
                continue;
            }
            if recorded.uid != uid || recorded.gid != gid {
                found.push(candidate);
            }
        }
        found.sort();
        Ok(found)
    }
}

/// [`PartitionTables`] over recorded `sfdisk --json` documents.
#[derive(Debug, Clone, Default)]
pub struct FixtureTables {
    tables: BTreeMap<PathBuf, Option<SfdiskTable>>,
}

impl FixtureTables {
    /// No devices recorded; every read is absent.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a device's table.
    pub fn insert(&mut self, device: impl Into<PathBuf>, table: SfdiskTable) {
        self.tables.insert(device.into(), Some(table));
    }

    /// Record a device as not attached.
    pub fn absent(&mut self, device: impl Into<PathBuf>) {
        self.tables.insert(device.into(), None);
    }
}

impl PartitionTables for FixtureTables {
    fn read(&self, device: &Path) -> Result<TableRead> {
        match self.tables.get(device) {
            Some(Some(table)) => Ok(TableRead::Table(table.clone())),
            Some(None) | None => Ok(TableRead::Absent),
        }
    }
}

/// A [`Runner`] that records actions instead of running them.
#[derive(Debug, Default)]
pub struct RecordingRunner {
    actions: RefCell<Vec<Action>>,
}

impl RecordingRunner {
    /// The actions recorded so far, in order.
    #[must_use]
    pub fn actions(&self) -> Vec<Action> {
        self.actions.borrow().clone()
    }
}

impl Runner for RecordingRunner {
    fn run(&self, action: &Action) -> Result<()> {
        self.actions.borrow_mut().push(action.clone());
        Ok(())
    }
}

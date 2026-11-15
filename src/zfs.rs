//! ZFS facts, read through the `zfs` CLI's JSON output.
//!
//! OpenZFS 2.3 and later emit JSON for `zfs get -j` and `zfs list -j`. cdsk
//! parses those documents and never links libzfs: the [`Zfs`] trait is the
//! whole surface, so tests and the `--state` fixture stand in for a live pool.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::plan::{Action, capture};

/// One property as `zfs get -j` reports it: its value and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyValue {
    /// The property value, as ZFS words it.
    pub value: String,
    /// `local`, `default`, `inherited from ...`, `none` or `-`.
    #[serde(default)]
    pub source: Option<String>,
}

impl PropertyValue {
    /// Whether the property is set on the dataset itself.
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.source.as_deref() == Some("local")
    }
}

/// One dataset's properties.
pub type PropertyMap = BTreeMap<String, PropertyValue>;

/// Properties of several datasets, keyed by dataset name.
pub type PropertySets = BTreeMap<String, PropertyMap>;

/// A `zfs get -j` or `zfs list -j` document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZfsJson {
    /// Entries keyed by dataset (or snapshot) name.
    #[serde(default)]
    pub datasets: BTreeMap<String, DatasetJson>,
}

/// One dataset or snapshot entry in a JSON document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetJson {
    /// The name, when the command prints it as a column.
    #[serde(default)]
    pub name: Option<String>,
    /// The dataset's properties.
    #[serde(default)]
    pub properties: PropertyMap,
}

impl ZfsJson {
    /// Parse a JSON document, naming it in the error.
    pub fn parse(text: &str, what: &str) -> Result<Self> {
        serde_json::from_str(text).map_err(|error| Error::json(what, error))
    }

    /// Every entry name, sorted.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.datasets.keys().cloned().collect()
    }

    /// The entries' properties, keyed by name.
    #[must_use]
    pub fn properties(&self) -> PropertySets {
        self.datasets
            .iter()
            .map(|(name, entry)| (name.clone(), entry.properties.clone()))
            .collect()
    }
}

/// Observed ZFS state, behind a trait so tests and `--state` use fixtures.
pub trait Zfs {
    /// Properties of `target`, and of its descendants when `recursive`.
    fn get(&self, target: &str, properties: &[&str], recursive: bool) -> Result<PropertySets>;

    /// Whether `dataset` exists.
    fn exists(&self, dataset: &str) -> Result<bool>;

    /// Dataset names at or beneath `root`.
    fn datasets(&self, root: &str) -> Result<Vec<String>>;

    /// Snapshot names of `dataset`, without the `@`.
    fn snapshots(&self, dataset: &str) -> Result<Vec<String>>;
}

/// The real thing: `zfs` with JSON output.
#[derive(Debug, Clone)]
pub struct CommandZfs {
    program: String,
}

impl CommandZfs {
    /// The `zfs` on `PATH`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_program("zfs")
    }

    /// A `zfs` at another path, for a host that does not use `PATH`.
    #[must_use]
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

impl Default for CommandZfs {
    fn default() -> Self {
        Self::new()
    }
}

impl Zfs for CommandZfs {
    fn get(&self, target: &str, properties: &[&str], recursive: bool) -> Result<PropertySets> {
        if properties.is_empty() {
            return Ok(PropertySets::new());
        }
        let mut argv = vec![
            self.program.clone(),
            "get".to_owned(),
            "-j".to_owned(),
            "-p".to_owned(),
        ];
        if recursive {
            argv.push("-r".to_owned());
        }
        argv.extend(properties.iter().map(|property| (*property).to_owned()));
        argv.push(target.to_owned());
        let text = capture(&argv)?;
        Ok(ZfsJson::parse(&text, "zfs get -j")?.properties())
    }

    fn exists(&self, dataset: &str) -> Result<bool> {
        let argv = vec![
            self.program.clone(),
            "list".to_owned(),
            "-j".to_owned(),
            "-d".to_owned(),
            "0".to_owned(),
            dataset.to_owned(),
        ];
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| Error::Refused("an empty command cannot be run".to_owned()))?;
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|error| Error::command(&argv, error.to_string()))?;
        if output.status.success() {
            return Ok(true);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("does not exist") {
            Ok(false)
        } else {
            Err(Error::Command {
                command: Action::new(argv).render(),
                message: stderr.trim().to_owned(),
            })
        }
    }

    fn datasets(&self, root: &str) -> Result<Vec<String>> {
        let argv = vec![
            self.program.clone(),
            "list".to_owned(),
            "-j".to_owned(),
            "-r".to_owned(),
            "-t".to_owned(),
            "filesystem".to_owned(),
            root.to_owned(),
        ];
        let text = capture(&argv)?;
        Ok(ZfsJson::parse(&text, "zfs list -j")?.names())
    }

    fn snapshots(&self, dataset: &str) -> Result<Vec<String>> {
        let argv = vec![
            self.program.clone(),
            "list".to_owned(),
            "-j".to_owned(),
            "-r".to_owned(),
            "-d".to_owned(),
            "1".to_owned(),
            "-t".to_owned(),
            "snapshot".to_owned(),
            dataset.to_owned(),
        ];
        let text = capture(&argv)?;
        let prefix = format!("{dataset}@");
        let mut names: Vec<String> = ZfsJson::parse(&text, "zfs list -j")?
            .names()
            .into_iter()
            .filter_map(|name| name.strip_prefix(&prefix).map(str::to_owned))
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }
}

//! The declaration cdsk converges: `nixstorage`'s shape and ownership model,
//! as JSON.
//!
//! The Nix side resolves identities (a `nixiam` name to a uid/gid) and renders
//! this document; cdsk consumes names and numbers only, so it holds no host
//! data and no identity table of its own.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::{Error, Result};

/// Everything `nixstorage` declares that cdsk acts on.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Declaration {
    /// Dataset classes, by name.
    #[serde(default)]
    pub classes: BTreeMap<String, ShapeClass>,
    /// Datasets, keyed by their full ZFS name.
    #[serde(default)]
    pub datasets: BTreeMap<String, ShapeDataset>,
    /// Ownership and top-directory mode.
    #[serde(default)]
    pub reconciler: Reconciler,
}

/// One dataset class: the ZFS properties every dataset of the class converges
/// to.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShapeClass {
    /// The class's recordsize, if it has one.
    #[serde(default)]
    pub recordsize: Option<String>,
    /// The class's compression algorithm, if it has one.
    #[serde(default)]
    pub compression: Option<String>,
    /// Any other `zfs set`-able property.
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
}

/// How convergence treats a dataset's children.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Children {
    /// Never descend (the default).
    #[default]
    Ignore,
    /// Set the properties once and let ZFS inheritance cover descendants.
    Inherit,
    /// Descend and re-apply the class recursively on every run.
    Reconcile,
}

/// A dataset and the class it converges to.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShapeDataset {
    /// The class name; `null` names a dataset cdsk knows about but leaves out
    /// of property convergence.
    #[serde(default)]
    pub class: Option<String>,
    /// How convergence treats children.
    #[serde(default)]
    pub children: Children,
}

/// Ownership and top-directory mode declarations, the port of `reconcile.sh`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Reconciler {
    /// Tree roots, keyed by absolute path.
    #[serde(default)]
    pub roots: BTreeMap<String, Owned>,
    /// App leaves, keyed by absolute path.
    #[serde(default)]
    pub leaves: BTreeMap<String, Owned>,
    /// Extra paths no recursive pass may descend beneath.
    #[serde(default)]
    pub prune: Vec<String>,
}

/// One declared root or leaf.
#[derive(Debug, Clone, Deserialize)]
pub struct Owned {
    /// The uid the path converges to.
    pub uid: u32,
    /// The gid the path converges to.
    pub gid: u32,
    /// The top-directory mode, as four octal digits (for example `"2750"`).
    pub mode: String,
    /// `false` keeps the path declared and reported but never touched.
    #[serde(default = "yes")]
    pub reconcile: bool,
    /// Whether ownership is swept through every file underneath.
    #[serde(default)]
    pub recurse: bool,
}

fn yes() -> bool {
    true
}

impl Owned {
    /// The declared mode as bits.
    pub fn mode_bits(&self) -> Result<u32> {
        parse_mode(&self.mode)
    }
}

impl Declaration {
    /// The properties a declared dataset converges to, sorted by property
    /// name.
    pub fn class_properties(&self, dataset: &str) -> Result<BTreeMap<String, String>> {
        let entry = self
            .datasets
            .get(dataset)
            .ok_or_else(|| Error::Declaration(format!("dataset {dataset:?} is not declared")))?;
        let Some(class_name) = &entry.class else {
            return Ok(BTreeMap::new());
        };
        let class = self.classes.get(class_name).ok_or_else(|| {
            Error::Declaration(format!(
                "dataset {dataset:?} names class {class_name:?}, which is not declared"
            ))
        })?;

        let mut properties = BTreeMap::new();
        if let Some(value) = &class.recordsize {
            properties.insert("recordsize".to_owned(), value.clone());
        }
        if let Some(value) = &class.compression {
            properties.insert("compression".to_owned(), value.clone());
        }
        for (property, value) in &class.properties {
            validate_property(property, class_name)?;
            if let Some(previous) = properties.insert(property.clone(), value.clone())
                && &previous != value
            {
                return Err(Error::Declaration(format!(
                    "class {class_name:?} declares {property:?} twice with different values"
                )));
            }
        }
        Ok(properties)
    }
}

fn validate_property(property: &str, class_name: &str) -> Result<()> {
    if property.is_empty() || property.contains('=') || property.contains(char::is_whitespace) {
        return Err(Error::Declaration(format!(
            "class {class_name:?} declares an invalid property name {property:?}"
        )));
    }
    Ok(())
}

/// Parse a four-digit octal mode (`"0750"`, `"2750"`, `"0o750"`).
pub fn parse_mode(text: &str) -> Result<u32> {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix("0o").unwrap_or(trimmed);
    if digits.is_empty() {
        return Err(Error::Declaration(format!("mode {text:?} is empty")));
    }
    let bits = u32::from_str_radix(digits, 8)
        .map_err(|_| Error::Declaration(format!("mode {text:?} is not octal")))?;
    if bits > 0o7777 {
        return Err(Error::Declaration(format!(
            "mode {text:?} has bits outside 0o7777"
        )));
    }
    Ok(bits)
}

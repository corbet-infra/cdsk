//! Dataset shape convergence: compute the property diff and plan only the
//! `zfs set` calls that close it.
//!
//! Two rules from `nixstorage` are load-bearing here: convergence may set
//! properties on an existing dataset, and it must never create or destroy one.
//! A declared dataset that is absent is reported as drift and left alone.

use std::collections::BTreeMap;

use cdrv::{EventLog, Severity, Subject, Timestamp};
use serde_json::json;

use crate::decl::{Children, Declaration};
use crate::error::Result;
use crate::plan::{Action, Plan};
use crate::units::parse_size;
use crate::zfs::Zfs;

/// What shape convergence found and planned.
#[derive(Debug, Clone, Default)]
pub struct ShapeReport {
    /// Every property that differs from the declaration.
    pub property_changes: Vec<PropertyChange>,
    /// Declared datasets that are absent; convergence never creates them.
    pub missing: Vec<String>,
    /// Datasets that already matched.
    pub converged: usize,
}

/// One property whose observed state differs from the declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyChange {
    /// The dataset.
    pub dataset: String,
    /// The property name.
    pub property: String,
    /// The observed value, when the property was present.
    pub current: Option<String>,
    /// The declared value.
    pub desired: String,
}

/// Plan the shape half of a convergence pass.
pub fn converge(
    decl: &Declaration,
    zfs: &dyn Zfs,
    log: &mut EventLog,
    at: Timestamp,
) -> Result<(Plan, ShapeReport)> {
    let desired = desired_properties(decl, zfs)?;
    let mut plan = Plan::new();
    let mut report = ShapeReport::default();

    for (dataset, wanted) in desired {
        if !zfs.exists(&dataset)? {
            report.missing.push(dataset.clone());
            plan.emit(log.emit(
                at,
                "dataset.drift",
                Subject::Dataset(dataset),
                Severity::Warn,
                json!({
                    "reason": "missing",
                    "detail": "convergence never creates datasets",
                }),
            ));
            continue;
        }

        let names: Vec<&str> = wanted.keys().map(String::as_str).collect();
        let observed = zfs.get(&dataset, &names, false)?;
        let current = observed.get(&dataset);

        let mut drifted = false;
        for (property, want) in &wanted {
            let observed_value = current.and_then(|map| map.get(property));
            let needs_set = match observed_value {
                None => true,
                Some(value) => !value.is_local() || !values_equal(property, &value.value, want),
            };
            if !needs_set {
                continue;
            }
            drifted = true;
            let change = PropertyChange {
                dataset: dataset.clone(),
                property: property.clone(),
                current: observed_value.map(|value| value.value.clone()),
                desired: want.clone(),
            };
            plan.push(Action::new([
                "zfs".to_owned(),
                "set".to_owned(),
                format!("{property}={want}"),
                dataset.clone(),
            ]));
            plan.emit(log.emit(
                at,
                "dataset.drift",
                Subject::Dataset(dataset.clone()),
                Severity::Warn,
                json!({
                    "property": property,
                    "current": change.current,
                    "desired": want,
                    "source": observed_value.and_then(|value| value.source.clone()),
                }),
            ));
            report.property_changes.push(change);
        }

        if !drifted {
            report.converged += 1;
            plan.emit(log.emit(
                at,
                "dataset.converged",
                Subject::Dataset(dataset),
                Severity::Info,
                json!({ "properties": wanted.keys().collect::<Vec<_>>() }),
            ));
        }
    }

    Ok((plan, report))
}

fn desired_properties(
    decl: &Declaration,
    zfs: &dyn Zfs,
) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
    let mut desired = BTreeMap::new();
    for name in decl.datasets.keys() {
        let properties = decl.class_properties(name)?;
        if !properties.is_empty() {
            desired.insert(name.clone(), properties);
        }
    }
    for (name, dataset) in &decl.datasets {
        if dataset.children != Children::Reconcile || dataset.class.is_none() {
            continue;
        }
        let properties = decl.class_properties(name)?;
        if properties.is_empty() {
            continue;
        }
        for descendant in zfs.datasets(name)? {
            if descendant == *name || decl.datasets.contains_key(&descendant) {
                continue;
            }
            desired
                .entry(descendant)
                .or_insert_with(|| properties.clone());
        }
    }
    Ok(desired)
}

fn values_equal(property: &str, current: &str, desired: &str) -> bool {
    let current = current.trim();
    let desired = desired.trim();
    if current.eq_ignore_ascii_case(desired) {
        return true;
    }
    if is_size_property(property)
        && let (Some(left), Some(right)) = (parse_size(current), parse_size(desired))
    {
        return left == right;
    }
    false
}

fn is_size_property(property: &str) -> bool {
    matches!(property, "recordsize" | "volblocksize" | "recsize")
        || property.ends_with("quota")
        || property.ends_with("reservation")
}

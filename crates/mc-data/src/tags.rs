//! Tags: `data/minecraft/tags/<registry>/<tag>.json` -> ID lists for the `update_tags` packet.
//! We send static (built-in) registries and synchronized dynamic ones, non-empty only
//! (like `TagNetworkSerialization.serializeTagsToNetwork`).

use crate::{DynamicRegistry, StaticRegistry};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

type RawTags = HashMap<String, HashMap<String, Vec<(String, bool)>>>; // registry -> tag -> (entry, required)

pub fn build(
    tags_dir: &Path,
    statics: &HashMap<String, StaticRegistry>,
    dynamics: &[DynamicRegistry],
) -> Result<Vec<(String, Vec<(String, Vec<i32>)>)>> {
    // Known registry names (without the `minecraft:` prefix) used to tell where the registry name ends and the tag begins.
    let mut known: HashSet<String> = statics.keys().map(|k| k.trim_start_matches("minecraft:").to_owned()).collect();
    for d in dynamics {
        known.insert(d.name.trim_start_matches("minecraft:").to_owned());
    }
    // Registries with tags that are not networked (e.g. worldgen/structure) are recognised too, so they can be skipped.
    for extra in ["worldgen/structure", "worldgen/world_preset", "worldgen/flat_level_generator_preset"] {
        known.insert(extra.to_owned());
    }

    let mut raw: RawTags = HashMap::new();
    let mut files = Vec::new();
    walk(tags_dir, &mut files)?;
    for path in files {
        let rel = path.strip_prefix(tags_dir)?.to_string_lossy().replace('\\', "/");
        let rel = rel.strip_suffix(".json").unwrap_or(&rel);
        // longest matching prefix = registry
        let mut best: Option<&String> = None;
        for k in &known {
            if rel.starts_with(&format!("{k}/")) && best.is_none_or(|b| k.len() > b.len()) {
                best = Some(k);
            }
        }
        let Some(reg) = best else { continue };
        let tag = &rel[reg.len() + 1..];
        let v: Value = serde_json::from_slice(&std::fs::read(&path)?).with_context(|| path.display().to_string())?;
        let values = v["values"].as_array().context("tag values")?;
        let mut list = Vec::new();
        for val in values {
            match val {
                Value::String(s) => list.push((s.clone(), true)),
                Value::Object(o) => list.push((o["id"].as_str().context("tag id")?.to_owned(), o.get("required").and_then(|r| r.as_bool()).unwrap_or(true))),
                _ => {}
            }
        }
        raw.entry(format!("minecraft:{reg}")).or_default().insert(format!("minecraft:{tag}"), list);
    }

    // ID lookup
    let dyn_ids: HashMap<&str, HashMap<&str, i32>> = dynamics
        .iter()
        .map(|d| (d.name.as_str(), d.entries.iter().enumerate().map(|(i, e)| (e.id.as_str(), i as i32)).collect()))
        .collect();
    let lookup = |reg: &str, name: &str| -> Option<i32> {
        if let Some(r) = statics.get(reg) {
            return r.ids.get(name).copied();
        }
        dyn_ids.get(reg)?.get(name).copied()
    };
    let networkable = |reg: &str| statics.contains_key(reg) || dynamics.iter().any(|d| d.name == reg);

    let mut out: BTreeMap<String, Vec<(String, Vec<i32>)>> = BTreeMap::new();
    for (reg, tags) in &raw {
        if !networkable(reg) {
            continue;
        }
        let mut names: Vec<&String> = tags.keys().collect();
        names.sort();
        let mut list = Vec::new();
        for name in names {
            let mut ids = Vec::new();
            let mut seen = HashSet::new();
            resolve(reg, name, tags, &lookup, &mut ids, &mut seen, &mut HashSet::new());
            list.push((name.clone(), ids));
        }
        if !list.is_empty() {
            out.insert(reg.clone(), list);
        }
    }
    Ok(out.into_iter().collect())
}

fn resolve(
    reg: &str,
    tag: &str,
    tags: &HashMap<String, Vec<(String, bool)>>,
    lookup: &dyn Fn(&str, &str) -> Option<i32>,
    out: &mut Vec<i32>,
    seen: &mut HashSet<i32>,
    visiting: &mut HashSet<String>,
) {
    if !visiting.insert(tag.to_owned()) {
        return; // cykl
    }
    if let Some(entries) = tags.get(tag) {
        for (e, _required) in entries {
            if let Some(r) = e.strip_prefix('#') {
                resolve(reg, r, tags, lookup, out, seen, visiting);
            } else if let Some(id) = lookup(reg, e) {
                if seen.insert(id) {
                    out.push(id);
                }
            }
        }
    }
    visiting.remove(tag);
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            walk(&e.path(), out)?;
        } else {
            out.push(e.path());
        }
    }
    Ok(())
}

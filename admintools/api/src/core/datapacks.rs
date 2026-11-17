//! What the world's datapacks add: their structures (`data/<ns>/worldgen/structure/*.json`),
//! read from pack folders and `.zip` packs in `<world>/datapacks`. Also the parsing of
//! `/locate structure` answers, which the structure search runs for each of them.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// One structure a pack defines.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StructureDef {
    /// `mvs:village_snowy` (namespace:path, no `.json`).
    pub id: String,
    /// The pack folder or zip it came from.
    pub pack: String,
}

/// The structure id for a path inside a pack (`data/ns/worldgen/structure/a/b.json` ->
/// `ns:a/b`), None for any other file.
pub fn structure_id(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    let p = p.strip_prefix("./").unwrap_or(&p);
    let rest = p.strip_prefix("data/")?;
    let (ns, rest) = rest.split_once('/')?;
    let rest = rest.strip_prefix("worldgen/structure/")?;
    let name = rest.strip_suffix(".json")?;
    if ns.is_empty() || name.is_empty() {
        return None;
    }
    Some(format!("{ns}:{name}"))
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > 12 {
        return;
    }
    let Ok(read) = fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, base, out, depth + 1);
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Every file path of a pack folder, relative to it, `/`-separated (only under `data/`).
fn folder_files(pack: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let data = pack.join("data");
    if data.is_dir() {
        walk(&data, pack, &mut out, 0);
    }
    out
}

/// The structures of every pack in `datapacks` (folders, and zips through `zip_names`, which
/// lists a zip's entries; the game glue passes the zip crate's reader). Sorted by id, one entry
/// per id (the last pack in name order wins, as packs load in order).
pub fn structures(datapacks: &Path, zip_names: &dyn Fn(&Path) -> Vec<String>) -> Vec<StructureDef> {
    let mut by_id: BTreeMap<String, String> = BTreeMap::new();
    let Ok(read) = fs::read_dir(datapacks) else { return Vec::new() };
    let mut packs: Vec<_> = read.flatten().map(|e| e.path()).collect();
    packs.sort();
    for pack in packs {
        let name = pack.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let files = if pack.is_dir() {
            folder_files(&pack)
        } else if name.to_ascii_lowercase().ends_with(".zip") {
            zip_names(&pack)
        } else {
            continue;
        };
        for f in files {
            if let Some(id) = structure_id(&f) {
                by_id.insert(id, name.clone());
            }
        }
    }
    by_id.into_iter().map(|(id, pack)| StructureDef { id, pack }).collect()
}

/// Whether `id` passes a search filter: empty, or a part of the id (`village`, `mvs:`).
pub fn matches(id: &str, filter: &str) -> bool {
    filter.is_empty() || id.contains(&filter.to_ascii_lowercase())
}

/// The block x and z of a `/tp @s X Y Z` suggestion (`/locate`'s link).
pub fn parse_tp(command: &str) -> Option<[i32; 2]> {
    let mut w = command.trim().trim_start_matches('/').split_whitespace();
    if !matches!(w.next(), Some("tp" | "teleport")) {
        return None;
    }
    let mut rest: Vec<&str> = w.collect();
    if rest.len() == 4 {
        rest.remove(0);
    }
    if rest.len() != 3 {
        return None;
    }
    Some([rest[0].parse().ok()?, rest[2].parse().ok()?])
}

/// The x and z of `/locate`'s plain answer: `... at [123, ~, -456] (789 blocks away)`.
pub fn parse_locate_plain(text: &str) -> Option<[i32; 2]> {
    let open = text.find('[')?;
    let close = text[open..].find(']')? + open;
    let parts: Vec<&str> = text[open + 1..close].split(',').map(str::trim).collect();
    if parts.len() != 3 {
        return None;
    }
    Some([parts[0].parse().ok()?, parts[2].parse().ok()?])
}

/// A found structure.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub id: String,
    pub x: i32,
    pub z: i32,
    pub distance: i32,
}

/// Horizontal distance, as `/locate` gives it.
pub fn distance(from: [i32; 2], to: [i32; 2]) -> i32 {
    let dx = f64::from(to[0] - from[0]);
    let dz = f64::from(to[1] - from[1]);
    (dx * dx + dz * dz).sqrt().floor() as i32
}

//! Regions: named boxes of one dimension with rules about mobs.
//!
//! Natural spawning (`NaturalSpawner`) asks, for each attempt, which mobs may appear at a
//! position of a category (`monster`, `creature`, `ambient`, `water_creature`, ...). A region
//! flag empties that list, so nothing of the category spawns there naturally, while spawners,
//! breeding, eggs and `/summon` still work. The `deny_*` flags and the `deny` list instead stop
//! any new entity of those kinds from joining the region (spawners and `/summon` included);
//! entities loaded with their chunk are never touched.
//!
//! Stored per world in `<world>/admintools/regions.json`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// The flags a region may carry, with what each does (shown by `/at region flags`).
pub const FLAGS: &[(&str, &str)] = &[
    ("no_monsters", "немає природного спавну ворожих мобів (категорія monster)"),
    ("no_animals", "немає природного спавну тварин (creature)"),
    ("no_ambient", "немає кажанів (ambient)"),
    ("no_water", "немає риб, кальмарів, аксолотлів, дельфінів (водні категорії)"),
    ("no_natural", "жодного природного спавну"),
    ("deny_hostile", "не пускати нових ворожих мобів ніяким способом (спавнери, /summon, яйця)"),
    ("deny_all_mobs", "не пускати жодних нових мобів, крім гравців, предметів і досвіду"),
];

/// Vanilla's hostile mob kinds, for `deny_hostile`.
pub const HOSTILE: &[&str] = &[
    "minecraft:blaze", "minecraft:bogged", "minecraft:breeze", "minecraft:cave_spider",
    "minecraft:creaking", "minecraft:creeper", "minecraft:drowned", "minecraft:elder_guardian",
    "minecraft:ender_dragon", "minecraft:enderman", "minecraft:endermite", "minecraft:evoker",
    "minecraft:ghast", "minecraft:guardian", "minecraft:hoglin", "minecraft:husk",
    "minecraft:illusioner", "minecraft:magma_cube", "minecraft:parched", "minecraft:phantom",
    "minecraft:piglin_brute", "minecraft:pillager", "minecraft:ravager", "minecraft:shulker",
    "minecraft:silverfish", "minecraft:skeleton", "minecraft:slime", "minecraft:spider",
    "minecraft:stray", "minecraft:vex", "minecraft:vindicator", "minecraft:warden",
    "minecraft:witch", "minecraft:wither", "minecraft:wither_skeleton", "minecraft:zoglin",
    "minecraft:zombie", "minecraft:zombie_villager", "minecraft:zombified_piglin",
    "minecraft:camel_husk",
];

/// Entity kinds that are never mobs: `deny_all_mobs` lets them in.
pub const NOT_MOBS: &[&str] = &[
    "minecraft:player", "minecraft:item", "minecraft:experience_orb", "minecraft:arrow",
    "minecraft:spectral_arrow", "minecraft:trident", "minecraft:snowball", "minecraft:egg",
    "minecraft:ender_pearl", "minecraft:potion", "minecraft:experience_bottle",
    "minecraft:firework_rocket", "minecraft:fishing_bobber", "minecraft:falling_block",
    "minecraft:tnt", "minecraft:area_effect_cloud", "minecraft:lightning_bolt",
    "minecraft:painting", "minecraft:item_frame", "minecraft:glow_item_frame",
    "minecraft:armor_stand", "minecraft:minecart", "minecraft:chest_minecart",
    "minecraft:hopper_minecart", "minecraft:furnace_minecart", "minecraft:tnt_minecart",
    "minecraft:command_block_minecart", "minecraft:spawner_minecart", "minecraft:leash_knot",
    "minecraft:end_crystal", "minecraft:eye_of_ender", "minecraft:marker",
    "minecraft:block_display", "minecraft:item_display", "minecraft:text_display",
    "minecraft:interaction", "minecraft:wind_charge", "minecraft:breeze_wind_charge",
    "minecraft:small_fireball", "minecraft:fireball", "minecraft:dragon_fireball",
    "minecraft:wither_skull", "minecraft:shulker_bullet", "minecraft:llama_spit",
    "minecraft:evoker_fangs", "minecraft:ominous_item_spawner",
];

/// Whether a flag stops natural spawns of `category` (`MobCategory`'s name).
pub fn flag_stops_category(flag: &str, category: &str) -> bool {
    match flag {
        "no_natural" => true,
        "no_monsters" => category == "monster",
        "no_animals" => category == "creature",
        "no_ambient" => category == "ambient",
        "no_water" => matches!(
            category,
            "water_creature" | "water_ambient" | "underground_water_creature" | "axolotls"
        ),
        _ => false,
    }
}

/// `zombie` and `minecraft:zombie` are the same kind.
pub fn full_id(kind: &str) -> String {
    let k = kind.trim().to_ascii_lowercase();
    if k.contains(':') { k } else { format!("minecraft:{k}") }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub name: String,
    /// `minecraft:overworld`.
    pub dimension: String,
    /// Inclusive block corners, min <= max on each axis.
    pub min: [i32; 3],
    pub max: [i32; 3],
    #[serde(default)]
    pub flags: BTreeSet<String>,
    /// Entity kinds (`minecraft:zombie`) that may not join the region.
    #[serde(default)]
    pub deny: BTreeSet<String>,
}

impl Region {
    /// A region between two corners in any order.
    pub fn new(name: &str, dimension: &str, a: [i32; 3], b: [i32; 3]) -> Region {
        Region {
            name: name.to_owned(),
            dimension: dimension.to_owned(),
            min: [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            max: [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
            flags: BTreeSet::new(),
            deny: BTreeSet::new(),
        }
    }

    pub fn contains_block(&self, dimension: &str, p: [i32; 3]) -> bool {
        self.dimension == dimension
            && (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }

    /// For entity positions: the block the point is in.
    pub fn contains(&self, dimension: &str, p: [f64; 3]) -> bool {
        self.contains_block(dimension, p.map(|v| v.floor() as i32))
    }

    pub fn size(&self) -> [i64; 3] {
        [0, 1, 2].map(|i| i64::from(self.max[i]) - i64::from(self.min[i]) + 1)
    }

    pub fn volume(&self) -> i64 {
        self.size().iter().product()
    }

    /// Whether the region's rules stop a new entity of `kind` from joining.
    pub fn denies(&self, kind: &str) -> bool {
        if kind == "minecraft:player" {
            return false;
        }
        if self.deny.contains(kind) {
            return true;
        }
        if self.flags.contains("deny_hostile") && HOSTILE.contains(&kind) {
            return true;
        }
        self.flags.contains("deny_all_mobs") && !NOT_MOBS.contains(&kind)
    }

    pub fn stops_natural(&self, category: &str) -> bool {
        self.flags.iter().any(|f| flag_stops_category(f, category))
    }

    pub fn describe(&self) -> String {
        let [sx, sy, sz] = self.size();
        let flags = if self.flags.is_empty() {
            "без прапорців".to_owned()
        } else {
            self.flags.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        let deny = if self.deny.is_empty() {
            String::new()
        } else {
            format!("; заборонені: {}", self.deny.iter().cloned().collect::<Vec<_>>().join(", "))
        };
        format!(
            "{} [{}] {} {} {} .. {} {} {} ({sx}x{sy}x{sz}): {flags}{deny}",
            self.name, short_dim(&self.dimension), self.min[0], self.min[1], self.min[2],
            self.max[0], self.max[1], self.max[2]
        )
    }
}

/// `minecraft:overworld` -> `overworld`.
pub fn short_dim(d: &str) -> &str {
    d.strip_prefix("minecraft:").unwrap_or(d)
}

/// A region name: 1..=32 of a-z, 0-9, `_`, `-`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// The full height of a world, for regions given only by x and z.
pub const FULL_HEIGHT: [i32; 2] = [-4096, 4096];

/// One coordinate: `12`, `-5`, `~` (where you stand) or `~-3`; `here` is None for the console.
pub fn parse_coord(w: &str, here: Option<i32>) -> Result<i32, String> {
    if let Some(rest) = w.strip_prefix('~') {
        let base = here.ok_or_else(|| format!("{w}: ~ працює лише з гри"))?;
        let off = if rest.is_empty() { 0.0 } else { rest.parse::<f64>().map_err(|_| format!("{w}: не число"))? };
        return Ok(base.saturating_add(off.floor() as i32));
    }
    let v = w.parse::<f64>().map_err(|_| format!("{w}: не координата"))?;
    if !v.is_finite() || v.abs() > 30_000_000.0 {
        return Err(format!("{w}: поза світом"));
    }
    Ok(v.floor() as i32)
}

fn coord_like(w: &str) -> bool {
    w.starts_with('~') || w.parse::<f64>().is_ok()
}

/// What `/at region create` was given.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateArgs {
    pub a: [i32; 3],
    pub b: [i32; 3],
    /// None: pick a free name ([`Regions::next_name`]).
    pub name: Option<String>,
}

/// `/at region create <x1 y1 z1> <x2 y2 z2> [назва]`, or `<x1 z1> <x2 z2> [назва]` for the full
/// height. Commas between numbers are fine (`10,64,-5`). `here` is the asker's block.
pub fn parse_create(args: &[&str], here: Option<[i32; 3]>) -> Result<CreateArgs, String> {
    let words: Vec<String> = args.iter().flat_map(|w| w.split(',')).filter(|w| !w.is_empty()).map(str::to_owned).collect();
    let n = words.iter().take_while(|w| coord_like(w)).count();
    let at = |i: usize, axis: usize| parse_coord(&words[i], here.map(|h| h[axis]));
    let (a, b) = match n {
        6 => ([at(0, 0)?, at(1, 1)?, at(2, 2)?], [at(3, 0)?, at(4, 1)?, at(5, 2)?]),
        4 => ([at(0, 0)?, FULL_HEIGHT[0], at(1, 2)?], [at(2, 0)?, FULL_HEIGHT[1], at(3, 2)?]),
        _ => {
            return Err("потрібні два кути: x1 y1 z1 x2 y2 z2 (або x1 z1 x2 z2 на всю висоту), ~ = де стоїш".to_owned());
        }
    };
    let rest = &words[n..];
    let name = match rest {
        [] => None,
        [name] => {
            let name = name.to_ascii_lowercase();
            if !valid_name(&name) {
                return Err("назва регіону: 1-32 символи a-z, 0-9, _ або -".to_owned());
            }
            Some(name)
        }
        _ => return Err(format!("зайве після назви: {}", rest[1..].join(" "))),
    };
    Ok(CreateArgs { a, b, name })
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Regions {
    pub regions: Vec<Region>,
}

pub const FILE: &str = "regions.json";

impl Regions {
    /// Reads `dir/regions.json`; a missing file is an empty list.
    pub fn load(dir: &Path) -> (Regions, Vec<String>) {
        let path = dir.join(FILE);
        match fs::read_to_string(&path) {
            Err(_) => (Regions::default(), Vec::new()),
            Ok(text) => match serde_json::from_str::<Regions>(&text) {
                Ok(r) => (r, Vec::new()),
                Err(e) => {
                    // Keep the broken file for the admin instead of overwriting it later.
                    let _ = fs::copy(&path, dir.join("regions.json.broken"));
                    (Regions::default(), vec![format!("{}: {e}; copied to regions.json.broken", path.display())])
                }
            },
        }
    }

    /// Writes `dir/regions.json` (through a temporary file, so a crash never leaves half of it).
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = dir.join("regions.json.tmp");
        fs::write(&tmp, text).map_err(|e| e.to_string())?;
        fs::rename(&tmp, dir.join(FILE)).map_err(|e| e.to_string())
    }

    /// The first free name `region1`, `region2`, ...
    pub fn next_name(&self) -> String {
        (1..).map(|i| format!("region{i}")).find(|n| self.get(n).is_none()).expect("endless")
    }

    /// Turns natural spawning of hostile mobs off (`no_monsters`) or back on; true when the
    /// region exists. Returns the new state.
    pub fn toggle_monsters(&mut self, name: &str) -> Option<bool> {
        let r = self.get_mut(name)?;
        let off = !r.flags.remove("no_monsters");
        if off {
            r.flags.insert("no_monsters".to_owned());
        }
        Some(off)
    }

    pub fn get(&self, name: &str) -> Option<&Region> {
        self.regions.iter().find(|r| r.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Region> {
        self.regions.iter_mut().find(|r| r.name == name)
    }

    /// Adds a region; a region of the same name is replaced (its flags kept).
    pub fn put(&mut self, mut region: Region) -> bool {
        if let Some(old) = self.get_mut(&region.name) {
            region.flags = std::mem::take(&mut old.flags);
            region.deny = std::mem::take(&mut old.deny);
            *old = region;
            false
        } else {
            self.regions.push(region);
            true
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.regions.len();
        self.regions.retain(|r| r.name != name);
        self.regions.len() != before
    }

    pub fn at(&self, dimension: &str, p: [i32; 3]) -> impl Iterator<Item = &Region> {
        self.regions.iter().filter(move |r| r.contains_block(dimension, p))
    }

    /// The region that stops a natural spawn of `category` at `p`, if any.
    pub fn stopping_natural(&self, dimension: &str, category: &str, p: [i32; 3]) -> Option<&Region> {
        self.at(dimension, p).find(|r| r.stops_natural(category))
    }

    /// The region that keeps a new entity of `kind` at `p` out, if any.
    pub fn denying(&self, dimension: &str, kind: &str, p: [f64; 3]) -> Option<&Region> {
        let b = p.map(|v| v.floor() as i32);
        self.at(dimension, b).find(|r| r.denies(kind))
    }

    /// Whether any region uses `deny` rules (the join hook can skip all work otherwise).
    pub fn any_deny(&self) -> bool {
        self.regions.iter().any(|r| {
            !r.deny.is_empty() || r.flags.contains("deny_hostile") || r.flags.contains("deny_all_mobs")
        })
    }

    pub fn any_natural(&self) -> bool {
        self.regions.iter().any(|r| r.flags.iter().any(|f| f.starts_with("no_")))
    }
}

//! AdminTools for Horizon: the server admin's toolbox, and the shared core of our mods.
//!
//! - Regions with mob rules: no natural spawn of a category, or no new mobs of some kinds at
//!   all (`/at region ...`).
//! - Profiler: what each step of the server tick costs, which mobs cost most, slow-tick spikes
//!   and plain-language reasons for lag (`/at prof`, `/at lag`, `/at tps`, `/at hotspots`,
//!   `/at spikes`).
//! - Nearest structures of the world's datapacks (`/at structures`).
//! - Test helpers: entity counts around you, clearing mobs/items, stress spawns, mod list.
//!
//! The rules live in `core/` (no game types, tested by `core-tests/`); this file is the glue
//! to Horizon's hooks. Other mods reuse the core through the trait mod dependency
//! `admintools = "^0.1"` and [`shared`].
//!
//! Work that needs the server's console (`/locate`, `/kill`, `/summon`) or counts players can't
//! run inside a player's command hook, so commands queue a [`Job`] that runs at the end of the
//! next server tick and answers with `tell`.

mod core;

use crate::core::config::{self, Config};
use crate::core::datapacks::{self, Found};
use crate::core::fmt;
use crate::core::profiler::{self, LevelCounts, Session, SpanSample};
use crate::core::regions::{self, Region, Regions};
use horizon_client::mod_api::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, RwLock};
use std::time::Instant;

/// Horizon's logger keeps Info/Warn only for targets starting with `horizon`.
const LOG: &str = "horizon::admintools";
const PREFIX: &str = "[AT] ";
const DIMENSIONS: [&str; 3] = ["minecraft:overworld", "minecraft:the_nether", "minecraft:the_end"];

const AQUA: u32 = 0x55FFFF;
const GREEN: u32 = 0x55FF55;
const RED: u32 = 0xFF5555;
const YELLOW: u32 = 0xFFFF55;
const GOLD: u32 = 0xFFAA00;
const GRAY: u32 = 0xAAAAAA;

// ---------------------------------------------------------------------------------------------
// Shared state, readable by other mods through `shared`.

static REGIONS: RwLock<Option<Regions>> = RwLock::new(None);
/// Whether any region has a natural-spawn flag / a deny rule: one load in the hot hooks.
static NATURAL_ON: AtomicBool = AtomicBool::new(false);
static DENY_ON: AtomicBool = AtomicBool::new(false);
/// The always-on spike log threshold in ms (0: off), f32 bits.
static SPIKE_LOG_MS: AtomicU32 = AtomicU32::new(0);

fn set_regions(r: Regions) {
    NATURAL_ON.store(r.any_natural(), Ordering::Relaxed);
    DENY_ON.store(r.any_deny(), Ordering::Relaxed);
    *REGIONS.write().unwrap_or_else(|p| p.into_inner()) = Some(r);
}

fn with_regions<T>(f: impl FnOnce(&Regions) -> T) -> Option<T> {
    let g = REGIONS.read().unwrap_or_else(|p| p.into_inner());
    g.as_ref().map(f)
}

/// What other mods may use (`admintools = "^0.1"` in their `[dependencies]`).
pub mod shared {
    pub use crate::core::config::load_toml;
    pub use crate::core::fmt;
    pub use crate::core::profiler::{Hotspot, hotspots, short, tick_time_stats};
    pub use crate::core::regions::{FLAGS, HOSTILE, Region, full_id};

    /// The names of the regions at a block.
    pub fn regions_at(dimension: &str, pos: [i32; 3]) -> Vec<String> {
        super::with_regions(|r| r.at(dimension, pos).map(|r| r.name.clone()).collect()).unwrap_or_default()
    }

    /// Whether a region at the block carries `flag` (`no_monsters`, or a mod's own flag set
    /// with `/at region flag <name> <flag> on`: any word is kept).
    pub fn has_flag(dimension: &str, pos: [i32; 3], flag: &str) -> bool {
        super::with_regions(|r| r.at(dimension, pos).any(|r| r.flags.contains(flag))).unwrap_or(false)
    }

    /// Whether natural spawning of `category` is off at the block.
    pub fn natural_blocked(dimension: &str, category: &str, pos: [i32; 3]) -> bool {
        super::with_regions(|r| r.stopping_natural(dimension, category, pos).is_some()).unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------------------------

pub fn hooks() -> Box<dyn Hooks> {
    Box::new(AdminTools { state: Mutex::new(State::default()), profile: Mutex::new(None) })
}

/// Who asked: a player (answered with `tell`) or the server console (the log).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Who {
    Player(u128),
    Console,
}

/// Where the asker stands.
#[derive(Debug, Clone)]
struct Place {
    dimension: String,
    pos: [f64; 3],
}

impl Place {
    fn block(&self) -> [i32; 3] {
        self.pos.map(|v| v.floor() as i32)
    }
}

#[derive(Debug, Clone)]
enum Job {
    Tps,
    Lag,
    Hotspots { dimension: String },
    Near { place: Place, radius: f64 },
    Structures { place: Place, filter: String },
    StructList { filter: String },
    Clear { place: Place, what: String, radius: f64 },
    Stress { place: Place, kind: String, count: u32 },
    ProfStart { seconds: u32 },
    ProfStop,
    RegionMenu { uuid: u128 },
}

struct Search {
    who: Who,
    place: Place,
    queue: Vec<String>,
    total: usize,
    found: Vec<Found>,
    started: Instant,
}

struct Profile {
    who: Who,
    session: Session,
    started: Instant,
    tick_n: u32,
    sampling: bool,
}

#[derive(Default)]
struct State {
    config: Config,
    opened: bool,
    world_dir: Option<PathBuf>,
    jobs: Vec<(Who, Job)>,
    /// Open region menus: player -> (window id, region name of each slot).
    menus: HashMap<u128, (i32, Vec<String>)>,
    search: Option<Search>,
}

struct AdminTools {
    state: Mutex<State>,
    /// Apart from `state`: the recording hooks fire inside the mod's own commands, and only
    /// ever `try_lock` it.
    profile: Mutex<Option<Profile>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while the lock was held must not take every later hook down with it.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn text(s: impl Into<String>, color: u32) -> Text {
    Text::colored(s.into(), color)
}

/// Answers `who`: a player in chat, the console in the log.
fn say(host: &mut dyn Host, who: Who, line: Text) {
    match who {
        Who::Player(uuid) => {
            host.tell(uuid, Text::colored(PREFIX, GOLD).append(line));
        }
        Who::Console => log::info!(target: LOG, "{PREFIX}{}", line.plain()),
    }
}

fn say_s(host: &mut dyn Host, who: Who, s: impl Into<String>, color: u32) {
    say(host, who, text(s, color));
}

/// A line with a teleport suggestion at its end.
fn with_tp(s: String, color: u32, x: i32, z: i32) -> Text {
    text(s, color).append(Text::literal(" ")).append(Text::teleport_link(x, None, z))
}

/// Regions shown in the menu (six rows).
const MENU_SLOTS: usize = 54;

fn region_item(r: &Region) -> ItemStack {
    let off = r.flags.contains("no_monsters");
    let mut s = ItemStack::new(if off { "minecraft:lime_concrete" } else { "minecraft:red_concrete" }, 1);
    let data = s.data_mut();
    let [sx, sy, sz] = r.size();
    let state = if off { ("Злі моби: НЕ спавняться", "green") } else { ("Злі моби: спавняться", "red") };
    data.set_loot_component("minecraft:custom_name", &serde_json::json!({ "text": r.name, "italic": false, "color": "yellow" }));
    let lore = serde_json::json!([
        { "text": state.0, "italic": false, "color": state.1 },
        { "text": format!("{} {} {} .. {} {} {}", r.min[0], r.min[1], r.min[2], r.max[0], r.max[1], r.max[2]), "italic": false, "color": "gray" },
        { "text": format!("{} ({sx}x{sy}x{sz})", regions::short_dim(&r.dimension)), "italic": false, "color": "gray" },
        { "text": "ЛКМ — перемкнути, ПКМ — телепорт", "italic": false, "color": "dark_gray" },
    ]);
    data.set_loot_component("minecraft:lore", &lore);
    s
}

fn regions_dir(world: &Option<PathBuf>) -> Option<PathBuf> {
    world.as_ref().map(|w| w.join("admintools"))
}

const HELP: &[&str] = &[
    "/at tps — TPS і час тіку за останні 100 тіків",
    "/at lag — швидкий знімок: TPS, сутності, блок-сутності по світах",
    "/at prof [секунди] — профайлер (за замовч. 30 с): кроки тіку, моби, стрибки, висновки",
    "/at prof stop — зупинити і показати звіт",
    "/at spikes <мс> — писати в лог кожен тік довший за N мс (0 = вимк.)",
    "/at hotspots — чанки з найбільшою кількістю сутностей і блок-сутностей",
    "/at near [радіус] — сутності навколо тебе",
    "/at structures [фільтр] — найближчі структури з датапаків",
    "/at structlist [фільтр] — які структури додають датапаки",
    "/at region — меню регіонів (спавн злих мобів), /at region help — команди регіонів",
    "/at clear <hostile|items|xp> [радіус] — прибрати навколо",
    "/at stress <кількість> <моб> — заспавнити мобів для тесту навантаження",
    "/at mods — список модів у збірці",
    "/at reload — перечитати конфіг і регіони",
];

const REGION_HELP: &[&str] = &[
    "/at region — меню регіонів: клік вмикає/вимикає спавн злих мобів",
    "/at region create <x1 y1 z1> <x2 y2 z2> [назва] — регіон між двома кутами (~ = де стоїш); злі моби в ньому одразу не спавняться",
    "/at region create <x1 z1> <x2 z2> [назва] — те саме на всю висоту",
    "/at region here <назва> <радіус> — квадрат навколо тебе на всю висоту",
    "/at region flag <назва> <прапорець> <on|off>",
    "/at region deny <назва> <моб> | allow <назва> <моб> — список заборонених мобів",
    "/at region list | info | flags | remove <назва>",
];

impl AdminTools {
    /// Reads the config and the world's regions (first tick, and `/at reload`).
    fn open(&self, host: &mut dyn Host) {
        let (config, problems) = match host.config_dir() {
            Ok(dir) => config::load(&dir),
            Err(e) => (Config::default(), vec![format!("no config folder: {e}")]),
        };
        for p in problems {
            log::warn!(target: LOG, "[AdminTools] admintools.toml: {p}");
        }
        let world = host.world_dir();
        let (regions, problems) = match regions_dir(&world) {
            Some(dir) => Regions::load(&dir),
            None => (Regions::default(), vec!["no world folder: regions are not kept".to_owned()]),
        };
        for p in problems {
            log::warn!(target: LOG, "[AdminTools] regions: {p}");
        }
        log::info!(target: LOG, "[AdminTools] {} region(s), commands /at (op level {})", regions.regions.len(), config.admin_level);
        SPIKE_LOG_MS.store(config.profiler.spike_log_ms.to_bits(), Ordering::Relaxed);
        set_regions(regions);
        let mut s = lock(&self.state);
        s.config = config;
        s.world_dir = world;
        s.opened = true;
    }

    fn save_regions(&self, host: &mut dyn Host, who: Who, regions: Regions) {
        let dir = regions_dir(&lock(&self.state).world_dir);
        let result = match &dir {
            Some(d) => regions.save(d),
            None => Err("немає папки світу".to_owned()),
        };
        set_regions(regions);
        if let Err(e) = result {
            say_s(host, who, format!("Не вдалося зберегти regions.json: {e}"), RED);
        }
    }

    // ----- commands --------------------------------------------------------------------------

    /// A `/at ...` line. `place` is None for the console.
    fn command(&self, host: &mut dyn Host, who: Who, place: Option<Place>, line: &str) {
        let words: Vec<&str> = line.split_whitespace().collect();
        let arg = |i: usize| words.get(i).copied().unwrap_or("");
        let num = |i: usize| words.get(i).and_then(|w| w.parse::<i64>().ok());
        let need_place = |host: &mut dyn Host| -> Option<Place> {
            if place.is_none() {
                say_s(host, who, "Ця команда тільки з гри (потрібна позиція гравця).", RED);
            }
            place.clone()
        };
        let queue = |job: Job| lock(&self.state).jobs.push((who, job));
        match arg(1) {
            "" | "help" => {
                for l in HELP {
                    say_s(host, who, *l, AQUA);
                }
            }
            "tps" => queue(Job::Tps),
            "lag" => queue(Job::Lag),
            "hotspots" => {
                let dimension = place.as_ref().map_or_else(|| DIMENSIONS[0].to_owned(), |p| p.dimension.clone());
                queue(Job::Hotspots { dimension });
            }
            "near" => {
                if let Some(place) = need_place(host) {
                    let radius = num(2).unwrap_or(48).clamp(1, 512) as f64;
                    queue(Job::Near { place, radius });
                }
            }
            "structures" | "structure" | "locate" => {
                if let Some(place) = need_place(host) {
                    queue(Job::Structures { place, filter: arg(2).to_ascii_lowercase() });
                }
            }
            "structlist" => queue(Job::StructList { filter: arg(2).to_ascii_lowercase() }),
            "clear" => {
                let what = arg(2).to_ascii_lowercase();
                if !matches!(what.as_str(), "hostile" | "items" | "xp") {
                    say_s(host, who, "Що прибрати: /at clear <hostile|items|xp> [радіус]", RED);
                } else if let Some(place) = need_place(host) {
                    let radius = num(3).unwrap_or(64).clamp(1, 512) as f64;
                    queue(Job::Clear { place, what, radius });
                }
            }
            "stress" => {
                let kind = arg(3);
                match (kind.is_empty(), num(2)) {
                    (false, Some(n)) if n > 0 => {
                        if let Some(place) = need_place(host) {
                            queue(Job::Stress { place, kind: regions::full_id(kind), count: n.min(500) as u32 });
                        }
                    }
                    _ => say_s(host, who, "Використання: /at stress <кількість до 500> <моб>", RED),
                }
            }
            "prof" | "profile" | "profiler" => {
                if arg(2) == "stop" {
                    queue(Job::ProfStop);
                } else {
                    let p = lock(&self.state).config.profiler.clone();
                    let seconds = num(2).map_or(p.default_seconds, |n| n.clamp(1, i64::from(p.max_seconds)) as u32);
                    queue(Job::ProfStart { seconds });
                }
            }
            "spikes" => match num(2) {
                Some(ms) if ms >= 0 => {
                    SPIKE_LOG_MS.store((ms.min(60_000) as f32).to_bits(), Ordering::Relaxed);
                    host.spans(ms > 0);
                    let line = if ms == 0 {
                        "Лог стрибків вимкнено.".to_owned()
                    } else {
                        format!("Кожен тік довший за {ms} мс тепер пишеться в лог сервера з найважчими кроками.")
                    };
                    say_s(host, who, line, GREEN);
                }
                _ => say_s(host, who, "Використання: /at spikes <мілісекунди>, 0 = вимкнути", RED),
            },
            "mods" => {
                let ids = mod_ids();
                say_s(host, who, format!("Моди в збірці ({}): {}", ids.len(), ids.join(", ")), AQUA);
            }
            "reload" => {
                self.open(host);
                let n = with_regions(|r| r.regions.len()).unwrap_or(0);
                say_s(host, who, format!("Конфіг і регіони перечитано ({n} регіонів)."), GREEN);
            }
            "region" | "regions" | "rg" => self.region(host, who, place.clone(), &words),
            other => say_s(host, who, format!("Невідома підкоманда {other}. /at help"), RED),
        }
    }

    fn region(&self, host: &mut dyn Host, who: Who, place: Option<Place>, w: &[&str]) {
        let arg = |i: usize| w.get(i).copied().unwrap_or("");
        let mut regions = with_regions(Clone::clone).unwrap_or_default();
        let sub = arg(2);
        let name = arg(3).to_ascii_lowercase();
        let needs_name = matches!(sub, "here" | "remove" | "flag" | "deny" | "allow");
        if needs_name && !regions::valid_name(&name) {
            say_s(host, who, "Назва регіону: 1-32 символи a-z, 0-9, _ або -.", RED);
            return;
        }
        let exists = |regions: &Regions, host: &mut dyn Host| {
            let ok = regions.get(&name).is_some();
            if !ok {
                say_s(host, who, format!("Немає регіону {name}. /at region list"), RED);
            }
            ok
        };
        match sub {
            "help" => {
                for l in REGION_HELP {
                    say_s(host, who, *l, AQUA);
                }
            }
            "" | "menu" | "gui" => match who {
                Who::Player(uuid) => lock(&self.state).jobs.push((who, Job::RegionMenu { uuid })),
                Who::Console => {
                    for l in REGION_HELP {
                        say_s(host, who, *l, AQUA);
                    }
                }
            },
            "create" | "add" => {
                let here = place.as_ref().map(Place::block);
                let args = match regions::parse_create(&w[3.min(w.len())..], here) {
                    Ok(a) => a,
                    Err(e) => {
                        say_s(host, who, format!("Не так: {e}."), RED);
                        say_s(host, who, "Приклад: /at region create ~-50 ~-20 ~-50 ~50 ~40 ~50 spawn", GRAY);
                        return;
                    }
                };
                let dimension = place.as_ref().map_or_else(|| DIMENSIONS[0].to_owned(), |p| p.dimension.clone());
                let name = args.name.unwrap_or_else(|| regions.next_name());
                let created = regions.put(Region::new(&name, &dimension, args.a, args.b));
                if created {
                    // What regions are for, for now: no hostile mobs from natural spawning.
                    regions.get_mut(&name).expect("just put").flags.insert("no_monsters".to_owned());
                }
                let line = regions.get(&name).map(Region::describe).unwrap_or_default();
                self.save_regions(host, who, regions);
                say_s(host, who, format!("Регіон {}: {line}", if created { "створено" } else { "оновлено (прапорці збережено)" }), GREEN);
                say_s(host, who, "Злі моби тут природно не спавняться. Перемкнути: /at region (меню).", GRAY);
            }
            "here" => {
                let Some(p) = place else {
                    say_s(host, who, "Тільки з гри.", RED);
                    return;
                };
                let Ok(radius) = arg(4).parse::<i32>() else {
                    say_s(host, who, "Використання: /at region here <назва> <радіус>", RED);
                    return;
                };
                let radius = radius.clamp(1, 30_000);
                let b = p.block();
                let r = Region::new(&name, &p.dimension, [b[0] - radius, -4096, b[2] - radius], [b[0] + radius, 4096, b[2] + radius]);
                let line = format!("Регіон {}: {}", if regions.put(r.clone()) { "створено" } else { "оновлено" }, r.describe());
                self.save_regions(host, who, regions);
                say_s(host, who, line, GREEN);
            }
            "remove" | "delete" => {
                if regions.remove(&name) {
                    self.save_regions(host, who, regions);
                    say_s(host, who, format!("Регіон {name} видалено."), GREEN);
                } else {
                    say_s(host, who, format!("Немає регіону {name}."), RED);
                }
            }
            "list" => {
                if regions.regions.is_empty() {
                    say_s(host, who, "Регіонів немає. /at region help", YELLOW);
                }
                for r in &regions.regions {
                    let c = [(r.min[0] + r.max[0]) / 2, (r.min[2] + r.max[2]) / 2];
                    say(host, who, with_tp(r.describe(), AQUA, c[0], c[1]));
                }
            }
            "info" => {
                let Some(p) = place else {
                    say_s(host, who, "Тільки з гри.", RED);
                    return;
                };
                let here: Vec<String> = regions.at(&p.dimension, p.block()).map(Region::describe).collect();
                if here.is_empty() {
                    say_s(host, who, "Ти не в регіоні.", YELLOW);
                }
                for l in here {
                    say_s(host, who, l, AQUA);
                }
            }
            "flags" => {
                for (f, d) in regions::FLAGS {
                    say_s(host, who, format!("{f}: {d}"), AQUA);
                }
            }
            "flag" => {
                if !exists(&regions, host) {
                    return;
                }
                let flag = arg(4).to_ascii_lowercase();
                let Some(on) = fmt::parse_bool(arg(5)) else {
                    say_s(host, who, "Використання: /at region flag <назва> <прапорець> <on|off>. Прапорці: /at region flags", RED);
                    return;
                };
                if flag.is_empty() || !flag.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                    say_s(host, who, "Прапорець: слово з a-z, 0-9, _.", RED);
                    return;
                }
                let known = regions::FLAGS.iter().any(|(f, _)| *f == flag);
                let r = regions.get_mut(&name).expect("checked");
                if on { r.flags.insert(flag.clone()); } else { r.flags.remove(&flag); }
                let line = r.describe();
                self.save_regions(host, who, regions);
                say_s(host, who, line, GREEN);
                if !known {
                    say_s(host, who, format!("{flag} не вбудований прапорець: його можуть читати інші моди (shared::has_flag)."), GRAY);
                }
            }
            "deny" | "allow" => {
                if !exists(&regions, host) {
                    return;
                }
                if arg(4).is_empty() {
                    say_s(host, who, format!("Використання: /at region {sub} <назва> <моб>"), RED);
                    return;
                }
                let kind = regions::full_id(arg(4));
                let r = regions.get_mut(&name).expect("checked");
                if sub == "deny" { r.deny.insert(kind); } else { r.deny.remove(&kind); }
                let line = r.describe();
                self.save_regions(host, who, regions);
                say_s(host, who, line, GREEN);
            }
            other => say_s(host, who, format!("Невідома дія {other}. /at region help"), RED),
        }
    }

    // ----- region menu -----------------------------------------------------------------------

    /// The chest menu of regions: one block per region, green when hostile mobs don't spawn
    /// there naturally, red when they do. A left click toggles it, a right click teleports.
    fn show_region_menu(&self, host: &mut dyn Host, uuid: u128) {
        let list: Vec<Region> = with_regions(|r| r.regions.clone()).unwrap_or_default();
        if list.is_empty() {
            say_s(host, Who::Player(uuid), "Регіонів ще немає. /at region create <x1 y1 z1> <x2 y2 z2> [назва]", YELLOW);
            return;
        }
        let shown: Vec<&Region> = list.iter().take(MENU_SLOTS).collect();
        let rows = shown.len().div_ceil(9).clamp(1, 6) as u8;
        let items: Vec<Option<ItemStack>> = (0..usize::from(rows) * 9).map(|i| shown.get(i).map(|r| region_item(r))).collect();
        let names: Vec<String> = shown.iter().map(|r| r.name.clone()).collect();
        let same = lock(&self.state).menus.get(&uuid).is_some_and(|(_, n)| n.len().div_ceil(9) == names.len().div_ceil(9));
        if same {
            host.set_menu_items(uuid, items);
            if let Some(m) = lock(&self.state).menus.get_mut(&uuid) {
                m.1 = names;
            }
        } else {
            // No lock held: opening closes the old menu, and its `menu_closed` comes in here.
            let title = Text::literal(format!("Регіони: спавн злих мобів ({})", list.len()));
            if let Some(window) = host.open_menu(uuid, rows, title, items) {
                lock(&self.state).menus.insert(uuid, (window, names));
            }
        }
        if list.len() > MENU_SLOTS {
            say_s(host, Who::Player(uuid), format!("У меню перші {MENU_SLOTS} регіонів; решта: /at region list"), GRAY);
        }
    }

    fn region_menu_click(&self, host: &mut dyn Host, uuid: u128, slot: usize, right: bool) {
        let name = lock(&self.state).menus.get(&uuid).and_then(|(_, n)| n.get(slot).cloned());
        let Some(name) = name else { return };
        let who = Who::Player(uuid);
        if right {
            // Right click: go there.
            let target = with_regions(|r| r.get(&name).cloned()).flatten();
            let player = host.server_players().into_iter().find(|p| p.uuid == uuid).map(|p| p.name);
            if let (Some(r), Some(player)) = (target, player) {
                let x = (r.min[0] + r.max[0]) / 2;
                let z = (r.min[2] + r.max[2]) / 2;
                host.close_menu(uuid);
                // On the ground in the region's middle; a plain tp if that column has no ground.
                let ground = format!("execute in {} run spreadplayers {x} {z} 0 1 false {player}", r.dimension);
                if !host.run_command(&ground) {
                    let y = r.max[1].clamp(-64, 320);
                    host.run_command(&format!("execute in {} run tp {player} {x}.5 {y} {z}.5", r.dimension));
                }
            }
            return;
        }
        let mut regions = with_regions(Clone::clone).unwrap_or_default();
        let Some(off) = regions.toggle_monsters(&name) else { return };
        self.save_regions(host, who, regions);
        let line = if off { format!("{name}: злі моби більше не спавняться природно.") } else { format!("{name}: злі моби знову спавняться.") };
        say_s(host, who, line, if off { GREEN } else { YELLOW });
        self.show_region_menu(host, uuid);
    }

    // ----- jobs (end of a server tick) --------------------------------------------------------

    fn run_job(&self, host: &mut dyn Host, who: Who, job: Job) {
        match job {
            Job::Tps => self.tps(host, who),
            Job::Lag => self.lag(host, who),
            Job::Hotspots { dimension } => self.hotspots(host, who, &dimension),
            Job::Near { place, radius } => self.near(host, who, &place, radius),
            Job::Structures { place, filter } => self.start_search(host, who, place, &filter),
            Job::StructList { filter } => {
                let list = self.datapack_structures();
                let list: Vec<_> = list.into_iter().filter(|s| datapacks::matches(&s.id, &filter)).collect();
                say_s(host, who, format!("Структур у датапаках: {}", list.len()), GOLD);
                let mut by_pack: Vec<(String, Vec<String>)> = Vec::new();
                for s in list {
                    match by_pack.iter_mut().find(|(p, _)| *p == s.pack) {
                        Some((_, ids)) => ids.push(s.id),
                        None => by_pack.push((s.pack, vec![s.id])),
                    }
                }
                for (pack, ids) in by_pack {
                    say_s(host, who, format!("{pack}: {}", ids.join(", ")), AQUA);
                }
            }
            Job::Clear { place, what, radius } => self.clear(host, who, &place, &what, radius),
            Job::Stress { place, kind, count } => {
                let [x, y, z] = place.pos;
                let mut ok = 0;
                for i in 0..count {
                    // A small spiral so they don't all stack in one block.
                    let a = i as f64 * 2.4;
                    let r = (i as f64).sqrt() * 0.8;
                    let line = format!(
                        "execute in {} run summon {kind} {:.2} {:.2} {:.2}",
                        place.dimension, x + r * a.cos(), y, z + r * a.sin()
                    );
                    if host.run_command(&line) {
                        ok += 1;
                    } else if ok == 0 {
                        break;
                    }
                }
                let color = if ok == count { GREEN } else { RED };
                say_s(host, who, format!("Заспавнено {ok} з {count} {kind}. Тепер /at prof 20, щоб побачити навантаження."), color);
            }
            Job::ProfStart { seconds } => self.prof_start(host, who, seconds),
            Job::RegionMenu { uuid } => self.show_region_menu(host, uuid),
            Job::ProfStop => {
                let running = lock(&self.profile).is_some();
                if running {
                    self.prof_finish(host);
                } else {
                    say_s(host, who, "Профайлер не запущений.", YELLOW);
                }
            }
        }
    }

    fn tps(&self, host: &mut dyn Host, who: Who) {
        let [t1, t5, t15] = host.tps();
        let (mean, p50, p95, max) = profiler::tick_time_stats(&host.tick_times());
        let (rate, _) = host.tick_stats();
        let budget = if rate > 0.0 { 1000.0 / rate } else { 50.0 };
        say(
            host,
            who,
            text("TPS ", GRAY)
                .append(text(format!("{t1:.1}"), fmt::tps_color(t1)))
                .append(text(format!(" / {t5:.1} / {t15:.1} (1, 5, 15 хв), ціль {rate:.0}"), GRAY)),
        );
        say(
            host,
            who,
            text("Тік (100 останніх): сер. ", GRAY)
                .append(text(format!("{mean:.1} мс"), fmt::mspt_color(mean)))
                .append(text(format!(", медіана {p50:.1}, p95 {p95:.1}, макс {max:.1} мс; бюджет {budget:.0} мс"), GRAY)),
        );
    }

    fn level_counts(&self, host: &mut dyn Host) -> Vec<LevelCounts> {
        DIMENSIONS
            .iter()
            .filter_map(|d| {
                host.level_stats(d).map(|s| LevelCounts {
                    dimension: (*d).to_owned(),
                    chunks: s.chunks,
                    forced_chunks: s.forced_chunks,
                    block_entities: s.block_entities,
                    ticking_block_entities: s.ticking_block_entities,
                    entities: s.entities.clone(),
                    block_entity_types: s.block_entity_types.clone(),
                })
            })
            .collect()
    }

    fn lag(&self, host: &mut dyn Host, who: Who) {
        self.tps(host, who);
        let players = host.server_players().len();
        say_s(host, who, format!("Гравців: {players}"), GRAY);
        for l in self.level_counts(host) {
            let ents = l.entities.iter().take(5).map(|(k, c)| format!("{} {c}", profiler::short(k))).collect::<Vec<_>>().join(", ");
            let bes = l.block_entity_types.iter().take(4).map(|(k, c)| format!("{} {c}", profiler::short(k))).collect::<Vec<_>>().join(", ");
            say_s(
                host,
                who,
                format!(
                    "{}: чанків {} (примус. {}), сутностей {} [{ents}], блок-сутностей {} (тікають {}) [{bes}]",
                    profiler::short(&l.dimension), l.chunks, l.forced_chunks, fmt::count(l.entity_total()),
                    fmt::count(l.block_entities), fmt::count(l.ticking_block_entities)
                ),
                AQUA,
            );
        }
        say_s(host, who, "Детальніше: /at prof 30 (що саме гальмує), /at hotspots (де).", GRAY);
    }

    fn hotspots(&self, host: &mut dyn Host, who: Who, dimension: &str) {
        let top = lock(&self.state).config.profiler.top;
        let ents: Vec<(String, [f64; 3])> = host.level_entities(dimension).into_iter().map(|(_, k, p)| (k, p)).collect();
        let bes = host.level_block_entities(dimension);
        say_s(host, who, format!("{}: сутностей {}, блок-сутностей {}", profiler::short(dimension), ents.len(), bes.len()), GOLD);
        say_s(host, who, "Чанки з найбільшою кількістю сутностей:", YELLOW);
        for h in profiler::hotspots(&ents, top) {
            let kinds = h.kinds.iter().map(|(k, c)| format!("{} {c}", profiler::short(k))).collect::<Vec<_>>().join(", ");
            let c = h.center();
            say(host, who, with_tp(format!("чанк {} {}: {} ({kinds})", h.chunk[0], h.chunk[1], h.total), AQUA, c[0], c[1]));
        }
        say_s(host, who, "Чанки з найбільшою кількістю блок-сутностей:", YELLOW);
        for h in profiler::block_hotspots(&bes, top) {
            let kinds = h.kinds.iter().map(|(k, c)| format!("{} {c}", profiler::short(k))).collect::<Vec<_>>().join(", ");
            let c = h.center();
            say(host, who, with_tp(format!("чанк {} {}: {} ({kinds})", h.chunk[0], h.chunk[1], h.total), AQUA, c[0], c[1]));
        }
    }

    fn entities_near(host: &mut dyn Host, place: &Place, radius: f64) -> Vec<(String, [f64; 3])> {
        let r2 = radius * radius;
        host.level_entities(&place.dimension)
            .into_iter()
            .filter(|(_, _, p)| {
                let d = [p[0] - place.pos[0], p[1] - place.pos[1], p[2] - place.pos[2]];
                d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= r2
            })
            .map(|(_, k, p)| (k, p))
            .collect()
    }

    fn near(&self, host: &mut dyn Host, who: Who, place: &Place, radius: f64) {
        let list = Self::entities_near(host, place, radius);
        let mut by: HashMap<&str, usize> = HashMap::new();
        for (k, _) in &list {
            *by.entry(k.as_str()).or_default() += 1;
        }
        let mut by: Vec<_> = by.into_iter().collect();
        by.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let line = by.iter().take(20).map(|(k, c)| format!("{} {c}", profiler::short(k))).collect::<Vec<_>>().join(", ");
        say_s(host, who, format!("У радіусі {radius:.0}: {} сутностей", list.len()), GOLD);
        if !line.is_empty() {
            say_s(host, who, line, AQUA);
        }
        let regions: Vec<String> = with_regions(|r| r.at(&place.dimension, place.block()).map(|r| r.name.clone()).collect()).unwrap_or_default();
        if !regions.is_empty() {
            say_s(host, who, format!("Ти в регіонах: {}", regions.join(", ")), GRAY);
        }
    }

    fn clear(&self, host: &mut dyn Host, who: Who, place: &Place, what: &str, radius: f64) {
        let kinds: Vec<String> = match what {
            "items" => vec!["minecraft:item".into()],
            "xp" => vec!["minecraft:experience_orb".into()],
            _ => {
                let mut k: Vec<String> = Self::entities_near(host, place, radius)
                    .into_iter()
                    .map(|(k, _)| k)
                    .filter(|k| regions::HOSTILE.contains(&k.as_str()) && k != "minecraft:ender_dragon" && k != "minecraft:wither")
                    .collect();
                k.sort();
                k.dedup();
                k
            }
        };
        let before = Self::entities_near(host, place, radius).len();
        let [x, y, z] = place.pos;
        for k in &kinds {
            let line = format!(
                "execute in {} positioned {x:.2} {y:.2} {z:.2} run kill @e[type={k},distance=..{radius:.0}]",
                place.dimension
            );
            host.run_command(&line);
        }
        let after = Self::entities_near(host, place, radius).len();
        say_s(host, who, format!("Прибрано {} сутностей ({what}, радіус {radius:.0}).", before.saturating_sub(after)), GREEN);
    }

    // ----- structures ------------------------------------------------------------------------

    fn datapack_structures(&self) -> Vec<datapacks::StructureDef> {
        let world = lock(&self.state).world_dir.clone();
        let Some(world) = world else { return Vec::new() };
        datapacks::structures(&world.join("datapacks"), &zip_names)
    }

    fn start_search(&self, host: &mut dyn Host, who: Who, place: Place, filter: &str) {
        if lock(&self.state).search.is_some() {
            say_s(host, who, "Пошук структур уже йде, зачекай.", YELLOW);
            return;
        }
        let ids: Vec<String> = self
            .datapack_structures()
            .into_iter()
            .map(|s| s.id)
            .filter(|id| datapacks::matches(id, filter))
            .collect();
        if ids.is_empty() {
            say_s(host, who, "У датапаках світу не знайдено структур (data/<ns>/worldgen/structure). /at structlist", YELLOW);
            return;
        }
        say_s(host, who, format!("Шукаю {} структур(и) в радіусі ~1600 блоків, по одній за тік...", ids.len()), GRAY);
        let total = ids.len();
        let mut queue = ids;
        queue.reverse();
        lock(&self.state).search = Some(Search { who, place, queue, total, found: Vec::new(), started: Instant::now() });
    }

    /// One `/locate` a tick, so a pack with many structures never stalls the server.
    fn step_search(&self, host: &mut dyn Host) {
        let Some(mut search) = lock(&self.state).search.take() else { return };
        if let Some(id) = search.queue.pop() {
            let b = search.place.block();
            let line = format!(
                "execute in {} positioned {} {} {} run locate structure {id}",
                search.place.dimension, b[0], b[1], b[2]
            );
            let result = host.run_command_captured(&line);
            if let Some([x, z]) = locate_answer(&result.output) {
                let distance = datapacks::distance([b[0], b[2]], [x, z]);
                search.found.push(Found { id, x, z, distance });
            }
        }
        if !search.queue.is_empty() {
            lock(&self.state).search = Some(search);
            return;
        }
        let max = lock(&self.state).config.structures.max_results;
        let who = search.who;
        search.found.sort_by_key(|f| f.distance);
        say_s(
            host,
            who,
            format!(
                "Знайдено {} з {} структур за {:.1} с. Найближчі:",
                search.found.len(), search.total, search.started.elapsed().as_secs_f32()
            ),
            GOLD,
        );
        for f in search.found.iter().take(max) {
            say(host, who, with_tp(format!("{} — {} блоків", f.id, f.distance), AQUA, f.x, f.z));
        }
        if search.found.len() < search.total {
            say_s(host, who, "Решти немає в радіусі пошуку /locate (100 чанків) або цей світ їх не генерує.", GRAY);
        }
    }

    // ----- profiler --------------------------------------------------------------------------

    fn prof_start(&self, host: &mut dyn Host, who: Who, seconds: u32) {
        let p = lock(&self.state).config.profiler.clone();
        let mut g = lock(&self.profile);
        if g.is_some() {
            drop(g);
            say_s(host, who, "Профайлер уже працює. /at prof stop", YELLOW);
            return;
        }
        *g = Some(Profile {
            who,
            session: Session::new(seconds, p.spike_ms, p.mob_sample_every),
            started: Instant::now(),
            tick_n: 0,
            sampling: false,
        });
        drop(g);
        host.spans(true);
        say_s(
            host,
            who,
            format!("Профайлер: записую {seconds} с. Грай як звичайно або відтвори лаг; звіт прийде сам. /at prof stop — раніше."),
            GREEN,
        );
    }

    fn prof_finish(&self, host: &mut dyn Host) {
        let Some(mut p) = lock(&self.profile).take() else { return };
        if p.sampling {
            host.mob_cost_sampling(false);
        }
        host.spans(SPIKE_LOG_MS.load(Ordering::Relaxed) != 0);
        p.session.elapsed_s = p.started.elapsed().as_secs_f32();
        p.session.levels = self.level_counts(host);
        p.session.players = host.server_players().len();
        let (top, world) = {
            let s = lock(&self.state);
            (s.config.profiler.top, s.world_dir.clone())
        };
        let r = p.session.report();
        let who = p.who;
        say(
            host,
            who,
            text(format!("Профіль {:.0} с: TPS {:.1}, тік сер. ", r.seconds, r.tps), GOLD)
                .append(text(format!("{:.1}", r.mean_ms), fmt::mspt_color(r.mean_ms)))
                .append(text(format!(" мс, p95 {:.1}, макс {:.0} мс, стрибків {}", r.p95_ms, r.max_ms, r.spike_count), GOLD)),
        );
        for v in &r.verdicts {
            say_s(host, who, format!("• {v}"), YELLOW);
        }
        say_s(host, who, "Кроки тіку (сер. мс, частка часу):", GRAY);
        for s in r.spans.iter().filter(|s| s.depth > 0).take(top) {
            say_s(host, who, format!("{} {}{} {:.2} мс (макс {:.0}) {:.0}%", fmt::bar(s.share), "  ".repeat(usize::from(s.depth.saturating_sub(1))), s.name, s.avg_ms, s.max_ms, s.share * 100.0), AQUA);
        }
        if !r.mobs.is_empty() {
            say_s(host, who, "Найдорожчі моби (мс на тік, скільки їх):", GRAY);
            for m in r.mobs.iter().take(top) {
                say_s(host, who, format!("{} {}: {:.2} мс, x{} ({:.0} мкс кожен)", profiler::short(&m.dimension), profiler::short(&m.kind), m.ms_per_tick, m.count, m.us_each), AQUA);
            }
        }
        let md = profiler::markdown(&r, "AdminTools: профіль сервера");
        match save_report(&world, &md) {
            Ok(path) => say_s(host, who, format!("Повний звіт: {}", path.display()), GRAY),
            Err(e) => say_s(host, who, format!("Звіт не збережено: {e}"), RED),
        }
        log::info!(target: LOG, "[AdminTools] profile done: {:.0} s, mean {:.1} ms, max {:.0} ms", r.seconds, r.mean_ms, r.max_ms);
    }
}

fn save_report(world: &Option<PathBuf>, md: &str) -> Result<PathBuf, String> {
    let dir = world.as_ref().ok_or("немає папки світу")?.join("admintools").join("profiles");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = dir.join(format!("profile-{secs}.md"));
    std::fs::write(&path, md).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The entries of a zip datapack.
fn zip_names(path: &Path) -> Vec<String> {
    let Ok(file) = std::fs::File::open(path) else { return Vec::new() };
    match zip::ZipArchive::new(std::io::BufReader::new(file)) {
        Ok(archive) => archive.file_names().filter_map(Result::ok).map(|n| n.into_owned()).collect(),
        Err(e) => {
            log::warn!(target: LOG, "[AdminTools] {}: {e}", path.display());
            Vec::new()
        }
    }
}

/// Where `/locate` found it: its link's `/tp` suggestion, else the coordinates in its text.
fn locate_answer(output: &[Text]) -> Option<[i32; 2]> {
    for t in output {
        for span in &t.spans {
            if let Some(horizon_client::text::ClickEvent::SuggestCommand(c)) = &span.style.click
                && let Some(xz) = datapacks::parse_tp(c)
            {
                return Some(xz);
            }
        }
        if let Some(xz) = datapacks::parse_locate_plain(&t.plain()) {
            return Some(xz);
        }
    }
    None
}

const USAGES: &[&str] = &[
    "at",
    "at help",
    "at tps",
    "at lag",
    "at hotspots",
    "at near [radius:integer]",
    "at structures [filter:text]",
    "at structlist [filter:text]",
    "at clear <what:word> [radius:integer]",
    "at stress <count:integer> <kind:text>",
    "at prof [seconds:integer]",
    "at prof stop",
    "at spikes <ms:integer>",
    "at mods",
    "at reload",
    "at region [action:word] [rest:text]",
];

impl Hooks for AdminTools {
    fn register_commands(&self, host: &mut dyn Host, event: &mut RegisterCommandsEvent) {
        // Asked when the world opens, before the first tick reads the config.
        let mut level = lock(&self.state).config.admin_level;
        if !lock(&self.state).opened
            && let Ok(dir) = host.config_dir()
        {
            level = config::load(&dir).0.admin_level;
        }
        let level = level.max(1);
        for u in USAGES {
            event.commands.push(NativeCommand::op(u, level));
        }
    }

    fn perform_command(&self, host: &mut dyn Host, event: &mut PerformCommandEvent) -> Flow {
        let line = event.command;
        if line.split_whitespace().next() != Some("at") {
            return Flow::Continue;
        }
        let level = lock(&self.state).config.admin_level;
        let p = event.player;
        if p.permission < level {
            host.feedback(text("Немає прав на /at.", RED));
            return Flow::Cancel;
        }
        let place = Place { dimension: p.dimension.clone(), pos: p.position };
        self.command(host, Who::Player(p.uuid), Some(place), line);
        Flow::Cancel
    }

    fn perform_console_command(&self, host: &mut dyn Host, event: &mut PerformConsoleCommandEvent) -> Flow {
        let line = event.command;
        if line.split_whitespace().next() != Some("at") {
            return Flow::Continue;
        }
        self.command(host, Who::Console, None, line);
        Flow::Cancel
    }

    fn server_tick(&self, host: &mut dyn Host, event: &mut ServerTickEvent) {
        if matches!(event.context.authority, NativeAuthority::RemoteClient) {
            return;
        }
        match event.phase {
            ServerTickPhase::ServerTickStart { .. } => {
                let mut g = lock(&self.profile);
                if let Some(p) = g.as_mut() {
                    p.tick_n += 1;
                    if p.session.samples_tick(p.tick_n) {
                        p.sampling = true;
                        p.session.sampled_ticks += 1;
                        drop(g);
                        host.mob_cost_sampling(true);
                    }
                }
            }
            ServerTickPhase::ServerTickEnd { .. } => {
                if !lock(&self.state).opened {
                    self.open(host);
                }
                // Mob costs of a sampled tick.
                let sampled = lock(&self.profile).as_mut().is_some_and(|p| std::mem::take(&mut p.sampling));
                if sampled {
                    host.mob_cost_sampling(false);
                    let costs = host.take_mob_costs();
                    if let Some(p) = lock(&self.profile).as_mut() {
                        for c in costs {
                            p.session.add_mob_cost(&c.dimension, &c.kind, c.nanos, c.mob_ticks);
                        }
                    }
                }
                let done = lock(&self.profile).as_mut().is_some_and(|p| {
                    p.session.elapsed_s = p.started.elapsed().as_secs_f32();
                    p.session.done()
                });
                if done {
                    self.prof_finish(host);
                }
                let jobs = std::mem::take(&mut lock(&self.state).jobs);
                for (who, job) in jobs {
                    self.run_job(host, who, job);
                }
                self.step_search(host);
            }
            _ => {}
        }
    }

    fn server_tick_timed(&self, _host: &mut dyn Host, event: &mut ServerTickTimedEvent) {
        let spike = f32::from_bits(SPIKE_LOG_MS.load(Ordering::Relaxed));
        if spike > 0.0 && event.wall_ms >= spike {
            let mut top: Vec<_> = event.spans.iter().filter(|s| s.depth > 0).collect();
            top.sort_by(|a, b| b.ms.total_cmp(&a.ms));
            let list = top.iter().take(6).map(|s| format!("{} {:.0}", s.name, s.ms)).collect::<Vec<_>>().join(", ");
            log::warn!(target: LOG, "[AdminTools] slow tick {:.0} ms: {list}", event.wall_ms);
        }
        let Ok(mut g) = self.profile.try_lock() else { return };
        if let Some(p) = g.as_mut() {
            p.session.elapsed_s = p.started.elapsed().as_secs_f32();
            let spans: Vec<SpanSample> = event
                .spans
                .iter()
                .map(|s| SpanSample { name: s.name, ms: s.ms, cpu_ms: s.cpu_ms, depth: s.depth })
                .collect();
            p.session.add_tick(event.wall_ms, event.cpu_ms, &spans);
        }
    }

    fn menu_clicked(&self, host: &mut dyn Host, event: &mut MenuClickedEvent) {
        let ours = lock(&self.state).menus.get(&event.uuid).is_some_and(|(w, _)| *w == event.window);
        if !ours {
            return;
        }
        // Only a plain left or right click counts; anything else just redraws (the client's copy
        // of a clicked slot must be put back).
        match usize::try_from(event.slot) {
            Ok(slot) if event.mode == 0 => self.region_menu_click(host, event.uuid, slot, event.button == 1),
            _ => self.show_region_menu(host, event.uuid),
        }
    }

    fn menu_closed(&self, _host: &mut dyn Host, event: &mut MenuClosedEvent) {
        let mut s = lock(&self.state);
        if s.menus.get(&event.uuid).is_some_and(|(w, _)| *w == event.window) {
            s.menus.remove(&event.uuid);
        }
    }

    fn potential_spawns(&self, _host: &dyn Host, event: &mut PotentialSpawnsEvent) -> Flow {
        if !NATURAL_ON.load(Ordering::Relaxed) {
            return Flow::Continue;
        }
        let stop = with_regions(|r| r.stopping_natural(event.dimension, event.category, event.pos).is_some()).unwrap_or(false);
        if stop { Flow::Cancel } else { Flow::Continue }
    }

    fn entity_join_level(&self, _host: &mut dyn Host, event: &mut EntityJoinLevelEvent) -> Flow {
        if !event.loaded_from_disk
            && let Ok(mut g) = self.profile.try_lock()
            && let Some(p) = g.as_mut()
        {
            p.session.add_join(event.kind);
        }
        if event.loaded_from_disk || !DENY_ON.load(Ordering::Relaxed) {
            return Flow::Continue;
        }
        let deny = with_regions(|r| r.denying(event.dimension, event.kind, event.pos).is_some()).unwrap_or(false);
        if deny { Flow::Cancel } else { Flow::Continue }
    }

    fn mob_spawned(&self, _host: &mut dyn Host, event: &mut MobSpawnedEvent) {
        if let Ok(mut g) = self.profile.try_lock()
            && let Some(p) = g.as_mut()
        {
            p.session.add_spawn(event.dimension, event.kind, event.natural);
        }
    }

    fn chunk_load(&self, _host: &mut dyn Host, event: &mut ChunkLoadEvent) {
        if let Ok(mut g) = self.profile.try_lock()
            && let Some(p) = g.as_mut()
        {
            p.session.add_chunk(event.dimension, event.new);
        }
    }
}

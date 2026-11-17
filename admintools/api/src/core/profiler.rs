//! The profiler's sums and verdicts. The game hands each server tick's timing (wall and CPU
//! time and its named steps, "spans": `tick`, `tick_mobs`, ...), sampled mob costs, spawns,
//! chunk loads and level counts to a [`Session`]; [`Session::report`] turns them into tables
//! and plain-language reasons for lag ([`Report::verdicts`]).

use std::collections::BTreeMap;

/// One named step of one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct SpanSample<'a> {
    pub name: &'a str,
    pub ms: f32,
    pub cpu_ms: f32,
    pub depth: u8,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpanSum {
    pub depth: u8,
    pub total_ms: f64,
    pub cpu_ms: f64,
    pub max_ms: f32,
    /// Ticks it appeared in.
    pub ticks: u32,
}

/// A slow tick: its time and slowest steps.
#[derive(Debug, Clone, PartialEq)]
pub struct Spike {
    /// Seconds since the session started.
    pub at_s: f32,
    pub ms: f32,
    pub top: Vec<(String, f32)>,
    /// New chunks that finished generating in that tick (the main thread waited for them).
    pub new_chunks: u32,
}

/// What one level held, read once at the end of a session (`native_level_stats`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelCounts {
    pub dimension: String,
    pub chunks: usize,
    pub forced_chunks: usize,
    pub block_entities: usize,
    pub ticking_block_entities: usize,
    pub entities: Vec<(String, usize)>,
    pub block_entity_types: Vec<(String, usize)>,
}

impl LevelCounts {
    pub fn entity_total(&self) -> usize {
        self.entities.iter().map(|e| e.1).sum()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub seconds: u32,
    pub spike_ms: f32,
    pub mob_sample_every: u32,
    /// Seconds since start, from the game's clock (set by the caller each tick).
    pub elapsed_s: f32,
    pub ticks: u32,
    pub wall: Vec<f32>,
    pub cpu_total_ms: f64,
    pub cpu_ticks: u32,
    pub spans: Vec<(String, SpanSum)>,
    pub spikes: Vec<Spike>,
    /// (dimension, mob kind) -> (nanoseconds, mob ticks), from sampled ticks only.
    pub mob_costs: BTreeMap<(String, String), (u64, u64)>,
    pub sampled_ticks: u32,
    /// (dimension, kind, natural) -> count.
    pub spawns: BTreeMap<(String, String, bool), u32>,
    /// dimension -> (new chunks generated, chunks read from disk).
    pub chunk_loads: BTreeMap<String, (u32, u32)>,
    /// Entity kind -> how many joined the world (fresh: drops, arrows, mobs...).
    pub joins: BTreeMap<String, u32>,
    pub levels: Vec<LevelCounts>,
    pub players: usize,
    /// New chunks since the last recorded tick.
    pub pending_new_chunks: u32,
    /// Ticks in which new chunks arrived: (ticks, wall ms summed); and the same for the others.
    pub gen_ticks: (u32, f64),
    pub calm_ticks: (u32, f64),
}

impl Session {
    pub fn new(seconds: u32, spike_ms: f32, mob_sample_every: u32) -> Session {
        Session { seconds, spike_ms, mob_sample_every: mob_sample_every.max(1), ..Session::default() }
    }

    /// Whether the session has run its time.
    pub fn done(&self) -> bool {
        self.elapsed_s >= self.seconds as f32
    }

    /// Whether mob costs should be timed in the tick numbered `n` of the session.
    pub fn samples_tick(&self, n: u32) -> bool {
        n % self.mob_sample_every == 0
    }

    pub fn add_tick(&mut self, wall_ms: f32, cpu_ms: Option<f32>, spans: &[SpanSample<'_>]) {
        self.ticks += 1;
        self.wall.push(wall_ms);
        let new_chunks = std::mem::take(&mut self.pending_new_chunks);
        let bucket = if new_chunks > 0 { &mut self.gen_ticks } else { &mut self.calm_ticks };
        bucket.0 += 1;
        bucket.1 += f64::from(wall_ms);
        if let Some(c) = cpu_ms {
            self.cpu_total_ms += f64::from(c);
            self.cpu_ticks += 1;
        }
        for s in spans {
            let i = match self.spans.iter().position(|(n, _)| n == s.name) {
                Some(i) => i,
                None => {
                    self.spans.push((s.name.to_owned(), SpanSum { depth: s.depth, ..SpanSum::default() }));
                    self.spans.len() - 1
                }
            };
            let sum = &mut self.spans[i].1;
            sum.total_ms += f64::from(s.ms);
            sum.cpu_ms += f64::from(s.cpu_ms);
            sum.max_ms = sum.max_ms.max(s.ms);
            sum.ticks += 1;
        }
        if wall_ms >= self.spike_ms && self.spikes.len() < 200 {
            let mut top: Vec<(String, f32)> = spans
                .iter()
                .filter(|s| s.depth > 0 || spans.len() == 1)
                .map(|s| (s.name.to_owned(), s.ms))
                .collect();
            if top.is_empty() {
                top = spans.iter().map(|s| (s.name.to_owned(), s.ms)).collect();
            }
            top.sort_by(|a, b| b.1.total_cmp(&a.1));
            top.truncate(5);
            self.spikes.push(Spike { at_s: self.elapsed_s, ms: wall_ms, top, new_chunks });
        }
    }

    pub fn add_mob_cost(&mut self, dimension: &str, kind: &str, nanos: u64, mob_ticks: u64) {
        let e = self.mob_costs.entry((dimension.to_owned(), kind.to_owned())).or_default();
        e.0 += nanos;
        e.1 += mob_ticks;
    }

    pub fn add_spawn(&mut self, dimension: &str, kind: &str, natural: bool) {
        *self.spawns.entry((dimension.to_owned(), kind.to_owned(), natural)).or_default() += 1;
    }

    pub fn add_chunk(&mut self, dimension: &str, new: bool) {
        let e = self.chunk_loads.entry(dimension.to_owned()).or_default();
        if new {
            e.0 += 1;
            self.pending_new_chunks += 1;
        } else {
            e.1 += 1;
        }
    }

    pub fn add_join(&mut self, kind: &str) {
        *self.joins.entry(kind.to_owned()).or_default() += 1;
    }

    pub fn report(&self) -> Report {
        let mut sorted = self.wall.clone();
        sorted.sort_by(f32::total_cmp);
        let n = sorted.len();
        let pct = |p: f64| -> f32 {
            if n == 0 { 0.0 } else { sorted[((p * (n - 1) as f64).round() as usize).min(n - 1)] }
        };
        let total: f64 = self.wall.iter().map(|&w| f64::from(w)).sum();
        let mean = if n == 0 { 0.0 } else { (total / n as f64) as f32 };
        let tick_total = total.max(1e-9);
        let mut spans: Vec<SpanRow> = self
            .spans
            .iter()
            .map(|(name, s)| SpanRow {
                name: name.clone(),
                depth: s.depth,
                avg_ms: if n == 0 { 0.0 } else { (s.total_ms / n as f64) as f32 },
                max_ms: s.max_ms,
                share: (s.total_ms / tick_total) as f32,
                cpu_share: if s.total_ms > 0.0 { (s.cpu_ms / s.total_ms) as f32 } else { 0.0 },
            })
            .collect();
        spans.sort_by(|a, b| b.avg_ms.total_cmp(&a.avg_ms));
        // Mob costs: sampled ticks only, so per-tick cost = sum / sampled ticks.
        let sampled = self.sampled_ticks.max(1) as f64;
        let mut mobs: Vec<MobRow> = self
            .mob_costs
            .iter()
            .map(|((dim, kind), (nanos, ticks))| MobRow {
                dimension: dim.clone(),
                kind: kind.clone(),
                ms_per_tick: (*nanos as f64 / 1e6 / sampled) as f32,
                count: (*ticks as f64 / sampled).round() as u64,
                us_each: if *ticks == 0 { 0.0 } else { (*nanos as f64 / *ticks as f64 / 1e3) as f32 },
            })
            .collect();
        mobs.sort_by(|a, b| b.ms_per_tick.total_cmp(&a.ms_per_tick));
        let mut spawns: Vec<(String, String, bool, u32)> = self
            .spawns
            .iter()
            .map(|((d, k, nat), c)| (d.clone(), k.clone(), *nat, *c))
            .collect();
        spawns.sort_by(|a, b| b.3.cmp(&a.3));
        let mut joins: Vec<(String, u32)> = self.joins.iter().map(|(k, c)| (k.clone(), *c)).collect();
        joins.sort_by(|a, b| b.1.cmp(&a.1));
        let avg = |(n, sum): (u32, f64)| if n == 0 { 0.0 } else { (sum / f64::from(n)) as f32 };
        let mut spikes = self.spikes.clone();
        spikes.sort_by(|a, b| b.ms.total_cmp(&a.ms));
        let mut r = Report {
            seconds: self.elapsed_s,
            ticks: n as u32,
            tps: if self.elapsed_s > 0.0 { n as f32 / self.elapsed_s } else { 0.0 },
            mean_ms: mean,
            p50_ms: pct(0.5),
            p95_ms: pct(0.95),
            p99_ms: pct(0.99),
            max_ms: sorted.last().copied().unwrap_or(0.0),
            cpu_share: if self.cpu_ticks > 0 && total > 0.0 { (self.cpu_total_ms / total) as f32 } else { -1.0 },
            spike_ms: self.spike_ms,
            spike_count: self.spikes.len(),
            spikes_with_gen: self.spikes.iter().filter(|s| s.new_chunks > 0).count(),
            gen_tick_ms: avg(self.gen_ticks),
            calm_tick_ms: avg(self.calm_ticks),
            gen_tick_count: self.gen_ticks.0,
            spikes,
            spans,
            mobs,
            spawns,
            chunk_loads: self.chunk_loads.iter().map(|(d, (a, b))| (d.clone(), *a, *b)).collect(),
            joins,
            levels: self.levels.clone(),
            players: self.players,
            verdicts: Vec::new(),
        };
        r.verdicts = verdicts(&r);
        r
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpanRow {
    pub name: String,
    pub depth: u8,
    pub avg_ms: f32,
    pub max_ms: f32,
    /// Its share of all tick time (0..1; nested spans count inside their parent too).
    pub share: f32,
    /// CPU time / wall time: near 1 is work, low means waiting (disk, locks, other threads).
    pub cpu_share: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MobRow {
    pub dimension: String,
    pub kind: String,
    pub ms_per_tick: f32,
    /// About how many of them ticked each tick.
    pub count: u64,
    /// Microseconds one of them costs a tick.
    pub us_each: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub seconds: f32,
    pub ticks: u32,
    pub tps: f32,
    pub mean_ms: f32,
    pub p50_ms: f32,
    pub p95_ms: f32,
    pub p99_ms: f32,
    pub max_ms: f32,
    /// Server thread CPU / wall over the session; -1 when the system doesn't tell.
    pub cpu_share: f32,
    pub spike_ms: f32,
    pub spike_count: usize,
    /// Spikes in a tick where new chunks arrived.
    pub spikes_with_gen: usize,
    /// Mean tick with new chunks arriving / without.
    pub gen_tick_ms: f32,
    pub calm_tick_ms: f32,
    pub gen_tick_count: u32,
    pub spikes: Vec<Spike>,
    pub spans: Vec<SpanRow>,
    pub mobs: Vec<MobRow>,
    pub spawns: Vec<(String, String, bool, u32)>,
    /// (dimension, generated, read from disk).
    pub chunk_loads: Vec<(String, u32, u32)>,
    pub joins: Vec<(String, u32)>,
    pub levels: Vec<LevelCounts>,
    pub players: usize,
    pub verdicts: Vec<String>,
}

/// The tick budget at 20 TPS.
pub const BUDGET_MS: f32 = 50.0;

fn span_like<'a>(r: &'a Report, words: &[&str]) -> Vec<&'a SpanRow> {
    r.spans
        .iter()
        .filter(|s| s.depth > 0 && words.iter().any(|w| s.name.contains(w)))
        .collect()
}

/// Whether the slow ticks come with chunk generation, which is where a datapack's terrain
/// (noise router, surface rules, structures, features) costs. Generation itself runs on worker
/// threads, so the game can't say which pack: the static analyser `tools/datapack_lag.py` can.
pub fn generation_verdict(r: &Report) -> Option<String> {
    let enough = r.gen_tick_count >= 20 && r.ticks > r.gen_tick_count;
    let slower = enough && r.calm_tick_ms > 0.0 && r.gen_tick_ms >= r.calm_tick_ms * 1.5 && r.gen_tick_ms - r.calm_tick_ms >= 3.0;
    let spikes = r.spike_count >= 3 && r.spikes_with_gen * 2 >= r.spike_count;
    if !slower && !spikes {
        return None;
    }
    let mut s = String::from("Лаг пов'язаний з генерацією нових чанків: ");
    if spikes {
        s += &format!("{} з {} стрибків припали на тіки, коли дозрівали нові чанки", r.spikes_with_gen, r.spike_count);
    }
    if slower {
        if spikes {
            s += "; ";
        }
        s += &format!(
            "тік з новими чанками в середньому {:.1} мс проти {:.1} мс без них ({} тіків)",
            r.gen_tick_ms, r.calm_tick_ms, r.gen_tick_count
        );
    }
    s += ". Найчастіше винен ландшафт датапака (і далекий ландшафт LOD рахується тим самим роутером): \
          запусти tools/datapack_lag.py на папці datapacks, він покаже, який пак і який файл важкий. \
          Перевірка в грі: /pregen з паком і без нього, порівняй швидкість.";
    Some(s)
}

/// Plain-language reasons for lag, most important first.
pub fn verdicts(r: &Report) -> Vec<String> {
    let mut v = Vec::new();
    if r.ticks == 0 {
        v.push("Немає даних: сервер не дав жодного тіку з таймінгами.".to_owned());
        return v;
    }
    if r.mean_ms < BUDGET_MS * 0.5 && r.max_ms < BUDGET_MS * 2.0 {
        v.push(format!(
            "Сервер у нормі: тік у середньому {:.1} мс з 50 мс бюджету, найдовший {:.0} мс.",
            r.mean_ms, r.max_ms
        ));
    } else if r.mean_ms >= BUDGET_MS {
        v.push(format!(
            "Сервер НЕ встигає: середній тік {:.1} мс > 50 мс, тому TPS {:.1} замість 20. Лагає постійно, не стрибками.",
            r.mean_ms, r.tps
        ));
    } else if r.p95_ms >= BUDGET_MS {
        v.push(format!(
            "Часті підвисання: 5% тіків довші за {:.0} мс (бюджет 50 мс), хоча середній тік {:.1} мс.",
            r.p95_ms, r.mean_ms
        ));
    }
    if r.spike_count > 0 {
        let mut causes: BTreeMap<&str, u32> = BTreeMap::new();
        for s in &r.spikes {
            if let Some((name, _)) = s.top.first() {
                *causes.entry(name.as_str()).or_default() += 1;
            }
        }
        let mut causes: Vec<_> = causes.into_iter().collect();
        causes.sort_by(|a, b| b.1.cmp(&a.1));
        let list = causes.iter().take(3).map(|(n, c)| format!("{n} ({c}x)")).collect::<Vec<_>>().join(", ");
        v.push(format!(
            "{} стрибків довших за {:.0} мс (найгірший {:.0} мс); найчастіше винен крок: {list}.",
            r.spike_count, r.spike_ms, r.max_ms
        ));
    }
    // Which kind of work dominates.
    let mob_ms: f32 = r.mobs.iter().map(|m| m.ms_per_tick).sum();
    if mob_ms > 0.0 && (mob_ms >= r.mean_ms * 0.3 || mob_ms > 10.0) {
        let top = r
            .mobs
            .iter()
            .take(3)
            .map(|m| format!("{} x{} ({:.1} мс)", short(&m.kind), m.count, m.ms_per_tick))
            .collect::<Vec<_>>()
            .join(", ");
        v.push(format!(
            "Моби забирають ~{mob_ms:.1} мс кожного тіку ({:.0}% часу). Найдорожчі: {top}. Ліки: регіон з no_monsters/deny_hostile, менший ліміт мобів (hub_server.toml), прибрати ферми з тисячами тварин.",
            100.0 * mob_ms / r.mean_ms.max(0.001)
        ));
    }
    let generated: u32 = r.chunk_loads.iter().map(|c| c.1).sum();
    if generated > 0 {
        let per_s = generated as f32 / r.seconds.max(1.0);
        if per_s >= 2.0 {
            v.push(format!(
                "Генерується {generated} нових чанків ({per_s:.1}/с): хтось летить елітрами або телепортується в нові місця. Ліки: заздалегідь згенерувати світ (/pregen start <радіус>), обмежити світ бордером."
            ));
        }
    }
    if let Some(v2) = generation_verdict(r) {
        v.push(v2);
    }
    // Time no named step covers: the engine does it inside a level's tick without a span of its
    // own (so the table can't say what it is), e.g. untimed per-entity or per-structure work.
    let top: f32 = r.spans.iter().filter(|s| s.depth == 0).map(|s| s.avg_ms).sum();
    let named: f32 = r.spans.iter().filter(|s| s.depth == 1).map(|s| s.avg_ms).sum();
    if top >= 20.0 && named < top * 0.4 {
        v.push(format!(
            "{:.0} мс з {:.0} мс тіку не покриває жоден названий крок (усі кроки разом {:.1} мс). Це робота рушія всередині тіку світу без власної мітки: таблиця кроків тут не допоможе. Перевір порівнянням: те саме місце з іншим набором датапаків, або інше місце (подалі від структур) з тими самими датапаками.",
            top - named, top, named
        ));
    }
    let total_ticking_be: usize = r.levels.iter().map(|l| l.ticking_block_entities).sum();
    if total_ticking_be > 3000 {
        let mut types: BTreeMap<&str, usize> = BTreeMap::new();
        for l in &r.levels {
            for (k, c) in &l.block_entity_types {
                *types.entry(k.as_str()).or_default() += c;
            }
        }
        let mut types: Vec<_> = types.into_iter().collect();
        types.sort_by(|a, b| b.1.cmp(&a.1));
        let top = types.iter().take(3).map(|(k, c)| format!("{} {c}", short(k))).collect::<Vec<_>>().join(", ");
        v.push(format!(
            "{total_ticking_be} блок-сутностей, що тікають (воронки, печі, спавнери): {top}. /at hotspots покаже чанки, де їх найбільше."
        ));
    }
    let entities: usize = r.levels.iter().map(LevelCounts::entity_total).sum();
    let items: usize = r
        .levels
        .iter()
        .flat_map(|l| l.entities.iter())
        .filter(|(k, _)| k == "minecraft:item" || k == "minecraft:experience_orb")
        .map(|(_, c)| c)
        .sum();
    if items > 1000 {
        v.push(format!(
            "На землі {items} предметів/кульок досвіду: десь працює ферма без збору. /at hotspots знайде місце, /at clear items прибере."
        ));
    }
    if entities > 5000 {
        v.push(format!("Всього {entities} сутностей у завантажених світах: це багато для одного сервера."));
    }
    for (needle, what) in [
        (&["save", "flush", "write"][..], "Збереження світу/чанків на диск"),
        (&["light"][..], "Перерахунок світла"),
        (&["redstone", "block_tick", "scheduled", "fluid"][..], "Редстоун, рідини і заплановані тіки блоків"),
        (&["net", "send", "packet", "track"][..], "Мережа: відправка чанків і сутностей гравцям"),
    ] {
        let rows = span_like(r, needle);
        let worst = rows.iter().map(|s| s.max_ms).fold(0.0f32, f32::max);
        let avg: f32 = rows.iter().map(|s| s.avg_ms).sum();
        if avg > r.mean_ms * 0.25 && avg > 5.0 || worst > 100.0 {
            v.push(format!("{what}: в середньому {avg:.1} мс на тік, найбільше {worst:.0} мс."));
        }
    }
    if r.cpu_share >= 0.0 && r.cpu_share < 0.6 && r.mean_ms > 25.0 {
        v.push(format!(
            "Потік сервера лише {:.0}% часу рахує, решту чекає (диск, пам'ять, інші потоки). Перевір вільну пам'ять і диск (на C: мало місця).",
            r.cpu_share * 100.0
        ));
    }
    if v.len() == 1 && r.mean_ms >= BUDGET_MS * 0.5 {
        if let Some(s) = r.spans.iter().find(|s| s.depth > 0) {
            v.push(format!("Найважчий крок тіку: {} ({:.1} мс у середньому).", s.name, s.avg_ms));
        }
    }
    v
}

/// `minecraft:zombie` -> `zombie`.
pub fn short(id: &str) -> &str {
    id.strip_prefix("minecraft:").unwrap_or(id)
}

/// Percentiles of the last ticks (`native_tick_times`, nanoseconds; zeros are empty slots):
/// (mean, p50, p95, max) in milliseconds.
pub fn tick_time_stats(nanos: &[u64]) -> (f32, f32, f32, f32) {
    let mut ms: Vec<f32> = nanos.iter().filter(|&&n| n > 0).map(|&n| n as f32 / 1e6).collect();
    if ms.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    ms.sort_by(f32::total_cmp);
    let n = ms.len();
    let mean = ms.iter().sum::<f32>() / n as f32;
    let at = |p: f32| ms[((p * (n - 1) as f32).round() as usize).min(n - 1)];
    (mean, at(0.5), at(0.95), ms[n - 1])
}

/// Entities grouped by chunk: the `top` chunks with the most, each with its leading kinds.
pub fn hotspots(entities: &[(String, [f64; 3])], top: usize) -> Vec<Hotspot> {
    let mut by_chunk: BTreeMap<(i32, i32), BTreeMap<&str, usize>> = BTreeMap::new();
    for (kind, p) in entities {
        if kind == "minecraft:player" {
            continue;
        }
        let c = ((p[0].floor() as i32) >> 4, (p[2].floor() as i32) >> 4);
        *by_chunk.entry(c).or_default().entry(kind.as_str()).or_default() += 1;
    }
    let mut out: Vec<Hotspot> = by_chunk
        .into_iter()
        .map(|(c, kinds)| {
            let total = kinds.values().sum();
            let mut kinds: Vec<(String, usize)> = kinds.into_iter().map(|(k, n)| (k.to_owned(), n)).collect();
            kinds.sort_by(|a, b| b.1.cmp(&a.1));
            kinds.truncate(3);
            Hotspot { chunk: [c.0, c.1], total, kinds }
        })
        .collect();
    out.sort_by(|a, b| b.total.cmp(&a.total).then(a.chunk.cmp(&b.chunk)));
    out.truncate(top);
    out
}

/// Block entities grouped the same way.
pub fn block_hotspots(blocks: &[([i32; 3], String)], top: usize) -> Vec<Hotspot> {
    let list: Vec<(String, [f64; 3])> = blocks
        .iter()
        .map(|(p, k)| (k.clone(), [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]))
        .collect();
    hotspots(&list, top)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hotspot {
    pub chunk: [i32; 2],
    pub total: usize,
    pub kinds: Vec<(String, usize)>,
}

impl Hotspot {
    /// The block in the chunk's middle, for a teleport link.
    pub fn center(&self) -> [i32; 2] {
        [self.chunk[0] * 16 + 8, self.chunk[1] * 16 + 8]
    }
}

/// The whole report as Markdown, for the file.
pub fn markdown(r: &Report, title: &str) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "# {title}\n");
    let _ = writeln!(
        s,
        "{:.0} с, {} тіків, {} гравців. TPS {:.2}. Тік: середній {:.2} мс, медіана {:.2}, p95 {:.2}, p99 {:.2}, макс {:.1} мс.{}\n",
        r.seconds, r.ticks, r.players, r.tps, r.mean_ms, r.p50_ms, r.p95_ms, r.p99_ms, r.max_ms,
        if r.cpu_share >= 0.0 { format!(" CPU потоку сервера: {:.0}% часу тіку.", r.cpu_share * 100.0) } else { String::new() }
    );
    let _ = writeln!(s, "## Висновки\n");
    for v in &r.verdicts {
        let _ = writeln!(s, "- {v}");
    }
    let _ = writeln!(s, "\n## Кроки тіку\n\n| крок | глибина | сер. мс | макс мс | частка | CPU/стіна |\n|---|---|---|---|---|---|");
    for x in &r.spans {
        let _ = writeln!(s, "| {}{} | {} | {:.2} | {:.1} | {:.0}% | {:.0}% |", "  ".repeat(x.depth as usize), x.name, x.depth, x.avg_ms, x.max_ms, x.share * 100.0, x.cpu_share * 100.0);
    }
    let _ = writeln!(s, "\n## Моби (час їхніх тіків)\n\n| світ | моб | мс/тік | скільки | мкс кожен |\n|---|---|---|---|---|");
    for m in &r.mobs {
        let _ = writeln!(s, "| {} | {} | {:.2} | {} | {:.1} |", short(&m.dimension), short(&m.kind), m.ms_per_tick, m.count, m.us_each);
    }
    let _ = writeln!(s, "\n## Стрибки (довші за {:.0} мс)\n", r.spike_ms);
    for sp in r.spikes.iter().take(50) {
        let top = sp.top.iter().map(|(n, ms)| format!("{n} {ms:.0}")).collect::<Vec<_>>().join(", ");
        let fresh = if sp.new_chunks > 0 { format!(", нових чанків: {}", sp.new_chunks) } else { String::new() };
        let _ = writeln!(s, "- {:.1} с: {:.0} мс ({top}{fresh})", sp.at_s, sp.ms);
    }
    if r.gen_tick_count > 0 {
        let _ = writeln!(
            s,
            "\nТіки, коли дозрівали нові чанки: {} (сер. {:.1} мс), решта: {} (сер. {:.1} мс). Стрибків під час генерації: {} з {}.",
            r.gen_tick_count,
            r.gen_tick_ms,
            r.ticks.saturating_sub(r.gen_tick_count),
            r.calm_tick_ms,
            r.spikes_with_gen,
            r.spike_count
        );
    }
    let _ = writeln!(s, "\n## Світи\n");
    for l in &r.levels {
        let _ = writeln!(
            s,
            "### {}\n\nЧанків {} (примусових {}), сутностей {}, блок-сутностей {} (тікають {}).\n",
            short(&l.dimension), l.chunks, l.forced_chunks, l.entity_total(), l.block_entities, l.ticking_block_entities
        );
        let ents = l.entities.iter().take(20).map(|(k, c)| format!("{} {c}", short(k))).collect::<Vec<_>>().join(", ");
        let bes = l.block_entity_types.iter().take(20).map(|(k, c)| format!("{} {c}", short(k))).collect::<Vec<_>>().join(", ");
        let _ = writeln!(s, "Сутності: {ents}\n\nБлок-сутності: {bes}\n");
    }
    let _ = writeln!(s, "## Чанки, що завантажились\n\n| світ | згенеровано | з диска |\n|---|---|---|");
    for (d, g, l) in &r.chunk_loads {
        let _ = writeln!(s, "| {} | {g} | {l} |", short(d));
    }
    let _ = writeln!(s, "\n## Спавн мобів за сесію\n\n| світ | моб | природний | скільки |\n|---|---|---|---|");
    for (d, k, nat, c) in r.spawns.iter().take(40) {
        let _ = writeln!(s, "| {} | {} | {} | {c} |", short(d), short(k), if *nat { "так" } else { "ні" });
    }
    let _ = writeln!(s, "\n## Нові сутності за сесію\n");
    let joins = r.joins.iter().take(30).map(|(k, c)| format!("{} {c}", short(k))).collect::<Vec<_>>().join(", ");
    let _ = writeln!(s, "{joins}");
    s
}

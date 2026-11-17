//! AdminTools' rules without the game: config, regions, profiler verdicts, datapack scanning.

use admintools_core_tests::core::{config, datapacks, profiler, regions};
use std::fs;
use std::path::PathBuf;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("admintools-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn config_is_written_with_defaults_and_read_back() {
    let d = temp("config");
    let (c, problems) = config::load(&d);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(c, config::Config::default());
    assert!(d.join("admintools.toml").is_file());
    fs::write(d.join("admintools.toml"), "admin_level = 3\n[profiler]\nspike_ms = 40\n").unwrap();
    let (c, _) = config::load(&d);
    assert_eq!(c.admin_level, 3);
    assert_eq!(c.profiler.spike_ms, 40.0);
    assert_eq!(c.profiler.default_seconds, 30, "missing keys keep their defaults");
}

#[test]
fn a_broken_or_out_of_range_config_falls_back() {
    let d = temp("badconfig");
    fs::write(d.join("admintools.toml"), "admin_level = \"x\"\n").unwrap();
    let (c, problems) = config::load(&d);
    assert_eq!(c, config::Config::default());
    assert_eq!(problems.len(), 1);
    fs::write(d.join("admintools.toml"), "admin_level = 9\n[profiler]\nmob_sample_every = 0\n").unwrap();
    let (c, problems) = config::load(&d);
    assert_eq!(c.admin_level, 4);
    assert_eq!(c.profiler.mob_sample_every, 4);
    assert_eq!(problems.len(), 2);
}

#[test]
fn region_corners_any_order_and_contains() {
    let r = regions::Region::new("spawn", "minecraft:overworld", [10, 80, -5], [-10, 60, 5]);
    assert_eq!(r.min, [-10, 60, -5]);
    assert_eq!(r.max, [10, 80, 5]);
    assert!(r.contains_block("minecraft:overworld", [10, 80, 5]));
    assert!(!r.contains_block("minecraft:overworld", [11, 80, 5]));
    assert!(!r.contains_block("minecraft:the_nether", [0, 70, 0]));
    assert!(r.contains("minecraft:overworld", [-9.5, 60.0, -4.2]));
    assert!(!r.contains("minecraft:overworld", [-10.5, 60.0, 0.0]), "-10.5 is block -11");
    assert_eq!(r.volume(), 21 * 21 * 11);
}

#[test]
fn flags_stop_their_categories_only() {
    let mut r = regions::Region::new("a", "minecraft:overworld", [0, 0, 0], [15, 255, 15]);
    r.flags.insert("no_monsters".into());
    assert!(r.stops_natural("monster"));
    assert!(!r.stops_natural("creature"));
    r.flags.insert("no_water".into());
    assert!(r.stops_natural("axolotls"));
    assert!(r.stops_natural("water_ambient"));
    r.flags.clear();
    r.flags.insert("no_natural".into());
    for c in ["monster", "creature", "ambient", "water_creature", "misc"] {
        assert!(r.stops_natural(c), "{c}");
    }
}

#[test]
fn deny_rules_never_touch_players() {
    let mut r = regions::Region::new("a", "minecraft:overworld", [0, 0, 0], [15, 255, 15]);
    r.flags.insert("deny_hostile".into());
    assert!(r.denies("minecraft:zombie"));
    assert!(!r.denies("minecraft:cow"));
    assert!(!r.denies("minecraft:player"));
    r.flags.clear();
    r.flags.insert("deny_all_mobs".into());
    assert!(r.denies("minecraft:cow"));
    assert!(r.denies("othermod:golem"));
    assert!(!r.denies("minecraft:item"));
    assert!(!r.denies("minecraft:arrow"));
    assert!(!r.denies("minecraft:player"));
    r.flags.clear();
    r.deny.insert(regions::full_id("Pig"));
    assert!(r.denies("minecraft:pig"));
}

#[test]
fn regions_save_load_and_lookup() {
    let d = temp("regions");
    let mut all = regions::Regions::default();
    let mut a = regions::Region::new("spawn", "minecraft:overworld", [-50, -64, -50], [50, 320, 50]);
    a.flags.insert("no_monsters".into());
    assert!(all.put(a));
    assert!(all.put(regions::Region::new("farm", "minecraft:overworld", [100, 0, 100], [120, 100, 120])));
    all.save(&d).unwrap();
    let (back, problems) = regions::Regions::load(&d);
    assert!(problems.is_empty());
    assert_eq!(back, all);
    assert_eq!(
        back.stopping_natural("minecraft:overworld", "monster", [0, 70, 0]).map(|r| r.name.as_str()),
        Some("spawn")
    );
    assert!(back.stopping_natural("minecraft:overworld", "creature", [0, 70, 0]).is_none());
    assert!(back.stopping_natural("minecraft:overworld", "monster", [110, 70, 110]).is_none());
    assert!(back.any_natural());
    assert!(!back.any_deny());
    // Re-creating keeps the flags.
    let mut again = back.clone();
    assert!(!again.put(regions::Region::new("spawn", "minecraft:overworld", [0, 0, 0], [1, 1, 1])));
    assert!(again.get("spawn").unwrap().flags.contains("no_monsters"));
    assert!(again.remove("farm"));
    assert!(!again.remove("farm"));
}

#[test]
fn a_broken_regions_file_is_kept_aside() {
    let d = temp("brokenregions");
    fs::write(d.join("regions.json"), "{ not json").unwrap();
    let (r, problems) = regions::Regions::load(&d);
    assert!(r.regions.is_empty());
    assert_eq!(problems.len(), 1);
    assert!(d.join("regions.json.broken").is_file());
}

#[test]
fn names() {
    assert!(regions::valid_name("spawn_1"));
    assert!(!regions::valid_name("Spawn"));
    assert!(!regions::valid_name(""));
    assert!(!regions::valid_name("a b"));
}

fn span(name: &str, ms: f32, depth: u8) -> profiler::SpanSample<'_> {
    profiler::SpanSample { name, ms, cpu_ms: ms * 0.9, depth }
}

#[test]
fn a_quiet_server_is_reported_healthy() {
    let mut s = profiler::Session::new(10, 60.0, 4);
    for i in 0..200 {
        s.elapsed_s = i as f32 / 20.0;
        s.add_tick(8.0, Some(7.0), &[span("tick", 8.0, 0), span("tick_mobs", 3.0, 1)]);
    }
    let r = s.report();
    assert_eq!(r.ticks, 200);
    assert!((r.mean_ms - 8.0).abs() < 1e-3);
    assert_eq!(r.spike_count, 0);
    assert!(r.verdicts[0].contains("у нормі"), "{:?}", r.verdicts);
    assert_eq!(r.spans[0].name, "tick");
    assert!((r.spans[1].share - 3.0 / 8.0).abs() < 1e-3);
}

#[test]
fn mobs_and_chunk_generation_are_named_as_causes() {
    let mut s = profiler::Session::new(10, 100.0, 4);
    for i in 0..100u32 {
        s.elapsed_s = i as f32 / 10.0;
        let slow = i % 10 == 0;
        let wall = if slow { 180.0 } else { 70.0 };
        s.add_tick(wall, Some(wall * 0.95), &[span("tick", wall, 0), span("tick_mobs", 40.0, 1), span("chunk_gen", if slow { 120.0 } else { 5.0 }, 1)]);
        if s.samples_tick(i) {
            s.sampled_ticks += 1;
            s.add_mob_cost("minecraft:overworld", "minecraft:zombie", 30_000_000, 600);
            s.add_mob_cost("minecraft:overworld", "minecraft:cow", 8_000_000, 400);
        }
    }
    for _ in 0..300 {
        s.add_chunk("minecraft:overworld", true);
    }
    let r = s.report();
    assert_eq!(r.spike_count, 10);
    assert_eq!(r.mobs[0].kind, "minecraft:zombie");
    assert!((r.mobs[0].ms_per_tick - 30.0).abs() < 0.01);
    assert_eq!(r.mobs[0].count, 600);
    let all = r.verdicts.join("\n");
    assert!(all.contains("НЕ встигає"), "{all}");
    assert!(all.contains("chunk_gen"), "spikes name their step: {all}");
    assert!(all.contains("Моби"), "{all}");
    assert!(all.contains("нових чанків"), "{all}");
    let md = profiler::markdown(&r, "test");
    assert!(md.contains("| overworld | zombie | 30.00 | 600 |"), "{md}");
}

#[test]
fn tick_time_stats_skip_empty_slots() {
    let mut t = vec![0u64; 100];
    for (i, v) in t.iter_mut().enumerate().take(10) {
        *v = (i as u64 + 1) * 1_000_000;
    }
    let (mean, p50, p95, max) = profiler::tick_time_stats(&t);
    assert!((mean - 5.5).abs() < 1e-3);
    assert_eq!(max, 10.0);
    assert!(p50 >= 5.0 && p50 <= 6.0);
    assert!(p95 >= 9.0);
    assert_eq!(profiler::tick_time_stats(&[0; 100]), (0.0, 0.0, 0.0, 0.0));
}

#[test]
fn hotspots_group_by_chunk() {
    let mut e = Vec::new();
    for i in 0..50 {
        e.push(("minecraft:cow".to_owned(), [1.0 + (i % 10) as f64, 64.0, 2.0]));
    }
    for _ in 0..5 {
        e.push(("minecraft:zombie".to_owned(), [-1.0, 64.0, -1.0]));
    }
    e.push(("minecraft:player".to_owned(), [-1.0, 64.0, -1.0]));
    let h = profiler::hotspots(&e, 10);
    assert_eq!(h.len(), 2);
    assert_eq!(h[0].chunk, [0, 0]);
    assert_eq!(h[0].total, 50);
    assert_eq!(h[1].chunk, [-1, -1]);
    assert_eq!(h[1].total, 5, "players don't count");
    assert_eq!(h[1].center(), [-8, -8]);
}

#[test]
fn structure_ids_from_pack_paths() {
    assert_eq!(datapacks::structure_id("data/mvs/worldgen/structure/village/snowy.json").as_deref(), Some("mvs:village/snowy"));
    assert_eq!(datapacks::structure_id("data\\dnt\\worldgen\\structure\\tavern.json").as_deref(), Some("dnt:tavern"));
    assert_eq!(datapacks::structure_id("data/mvs/worldgen/structure_set/villages.json"), None);
    assert_eq!(datapacks::structure_id("data/mvs/worldgen/structure/x.nbt"), None);
    assert_eq!(datapacks::structure_id("assets/mvs/worldgen/structure/x.json"), None);
}

#[test]
fn structures_from_folders_and_zips() {
    let d = temp("packs");
    let p = d.join("towers").join("data").join("tw").join("worldgen").join("structure");
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join("tower.json"), "{}").unwrap();
    fs::write(d.join("dungeons.zip"), "zip").unwrap();
    fs::write(d.join("readme.txt"), "x").unwrap();
    let zip = |path: &std::path::Path| {
        assert!(path.ends_with("dungeons.zip"));
        vec!["pack.mcmeta".to_owned(), "data/dg/worldgen/structure/crypt.json".to_owned()]
    };
    let s = datapacks::structures(&d, &zip);
    let ids: Vec<_> = s.iter().map(|s| (s.id.as_str(), s.pack.as_str())).collect();
    assert_eq!(ids, vec![("dg:crypt", "dungeons.zip"), ("tw:tower", "towers")]);
}

#[test]
fn locate_answers_parse() {
    assert_eq!(datapacks::parse_tp("/tp @s 123 ~ -456"), Some([123, -456]));
    assert_eq!(datapacks::parse_tp("tp 1 2 3"), Some([1, 3]));
    assert_eq!(datapacks::parse_tp("/give @s stone"), None);
    assert_eq!(
        datapacks::parse_locate_plain("The nearest mvs:x is at [123, ~, -456] (789 blocks away)"),
        Some([123, -456])
    );
    assert_eq!(datapacks::parse_locate_plain("Найближча mvs:x на [5, ~, 6]"), Some([5, 6]));
    assert_eq!(datapacks::parse_locate_plain("no structure"), None);
    assert_eq!(datapacks::distance([0, 0], [3, 4]), 5);
    assert!(datapacks::matches("mvs:village_snowy", "Village"));
}

#[test]
fn region_create_takes_two_corners_and_an_optional_name() {
    let here = Some([100, 64, -20]);
    let a = regions::parse_create(&["10", "60", "-5", "~10", "~", "~-5"], here).unwrap();
    assert_eq!((a.a, a.b, a.name), ([10, 60, -5], [110, 64, -25], None));
    let a = regions::parse_create(&["0,0", "50,50", "Spawn"], here).unwrap();
    assert_eq!(a.a, [0, regions::FULL_HEIGHT[0], 0]);
    assert_eq!(a.b, [50, regions::FULL_HEIGHT[1], 50]);
    assert_eq!(a.name.as_deref(), Some("spawn"));
    assert!(regions::parse_create(&["1", "2", "3"], here).is_err());
    assert!(regions::parse_create(&["~", "~", "~", "1", "2", "3"], None).is_err(), "~ needs a player");
    assert!(regions::parse_create(&["1", "2", "3", "4", "5", "6", "bad name!"], here).is_err());
    assert!(regions::parse_create(&["1", "2", "3", "4", "5", "6", "a", "b"], here).is_err());
}

#[test]
fn region_names_and_hostile_toggle() {
    let mut r = regions::Regions::default();
    assert_eq!(r.next_name(), "region1");
    r.put(regions::Region::new("region1", "minecraft:overworld", [0, 0, 0], [9, 9, 9]));
    assert_eq!(r.next_name(), "region2");
    assert_eq!(r.toggle_monsters("region1"), Some(true));
    assert!(r.stopping_natural("minecraft:overworld", "monster", [5, 5, 5]).is_some());
    assert_eq!(r.toggle_monsters("region1"), Some(false));
    assert!(r.stopping_natural("minecraft:overworld", "monster", [5, 5, 5]).is_none());
    assert_eq!(r.toggle_monsters("nope"), None);
}

#[test]
fn spikes_during_chunk_generation_point_at_datapacks() {
    let mut s = profiler::Session::new(30, 60.0, 4);
    for i in 0..400u32 {
        s.elapsed_s = i as f32 / 20.0;
        let fresh = i % 8 == 0;
        if fresh {
            for _ in 0..3 {
                s.add_chunk("minecraft:overworld", true);
            }
        }
        let wall = if fresh { 90.0 } else { 12.0 };
        s.add_tick(wall, None, &[span("tick", wall, 0), span("chunk_source", wall - 8.0, 1)]);
    }
    let r = s.report();
    assert_eq!(r.spike_count, 50);
    assert_eq!(r.spikes_with_gen, 50);
    assert_eq!(r.gen_tick_count, 50);
    assert!((r.gen_tick_ms - 90.0).abs() < 1e-3 && (r.calm_tick_ms - 12.0).abs() < 1e-3);
    let all = r.verdicts.join("\n");
    assert!(all.contains("генерацією нових чанків") && all.contains("datapack_lag.py"), "{all}");
    assert!(profiler::markdown(&r, "t").contains("нових чанків: 3"));
    // A calm server says nothing about generation.
    let mut q = profiler::Session::new(10, 60.0, 4);
    for _ in 0..100 {
        q.add_tick(10.0, None, &[span("tick", 10.0, 0)]);
    }
    assert!(profiler::generation_verdict(&q.report()).is_none());
}

#[test]
fn time_outside_every_named_step_is_called_out() {
    let mut s = profiler::Session::new(10, 200.0, 4);
    for _ in 0..100 {
        s.add_tick(120.0, None, &[span("level_overworld", 120.0, 0), span("tick_mobs", 4.0, 1), span("tick_lan", 2.0, 1)]);
    }
    let all = s.report().verdicts.join("\n");
    assert!(all.contains("не покриває жоден названий крок"), "{all}");
}

# AdminTools for Horizon

A server-side admin mod: regions with mob rules, a "why is it lagging" profiler, a nearest-structure finder
for data packs, test tools. Players install nothing. It is also the **shared core** of our other mods:
configs, regions, profiler and formatting are available to other mods.

All commands: `/at ...`, operator level 2 (configurable). Commands that don't need a player position also
work from the server console (`at tps`, `at lag`, `at prof 30`, `at region list`).

## Regions without hostile mob spawning

Create a region between two corners (`~` means "where I stand"):

```
/at region create 100 60 -200 180 120 -120 spawn
/at region create ~-50 ~-20 ~-50 ~50 ~40 ~50
/at region create -100 -100 100 100 spawn       (x z only: full height)
```

The name is optional (`region1`, `region2`...). In a new region hostile mobs immediately
**stop spawning naturally** (spawners, eggs and `/summon` still work, passive mobs still spawn).

`/at region` opens a menu: each region is a concrete block. Green: hostile mobs don't spawn,
red: they do. **LMB** toggles, **RMB** teleports to the region. Changes apply immediately and
are saved to `<world>/admintools/regions.json`.

Mobs already standing in the region don't disappear: remove them with `/at clear hostile 100`.

More (`/at region help`): `list`, `info`, `remove <name>`, `here <name> <radius>` (a square
around you), and for finer rules `flag <name> <flag> on|off` and `deny|allow <name> <mob>`:

| Flag | What it does |
|---|---|
| `no_monsters` | no natural spawning of hostile mobs (what the menu toggles) |
| `no_animals` | no natural spawning of animals |
| `no_ambient` | no bats |
| `no_water` | no water mobs |
| `no_natural` | no natural spawning at all |
| `deny_hostile` | hostile mobs don't appear in any way: no spawner, no `/summon`, no egg |
| `deny_all_mobs` | no new mobs at all (players, items, arrows, XP are fine) |

Any other flag word is stored too, and other mods can read it
(`admintools::shared::has_flag`).

## Why is it lagging: the profiler

| Command | What it shows |
|---|---|
| `/at tps` | TPS over 1/5/15 min, tick time: mean, median, p95, max |
| `/at lag` | a quick snapshot: TPS, chunks, entities and block entities per world |
| `/at prof 30` | a 30 s recording: tick phases (mobs, block entities, chunks, network...), the most expensive mobs, spikes, chunk generation, spawning; **conclusions in plain words** and a full report in `<world>/admintools/profiles/profile-*.md` |
| `/at hotspots` | chunks with the most entities and block entities, with a teleport link |
| `/at spikes 80` | a permanent log: every tick longer than 80 ms is written to the server log with the heaviest phases (0 = off) |

The profiler separately counts ticks in which new chunks finished generating. If the spikes fall on those,
the conclusions say "The lag is related to generating new chunks", and then the data pack's terrain is to blame
(see below).

How to catch lag in a scenario: run `/at prof 60`, reproduce the situation (elytra flight into new
land, a farm, a raid, many players) and wait for the report. For comparison, make the same recording while
idle. Load test: `/at stress 200 zombie`, then `/at prof 20`.

## Which data pack lags and what exactly in it

World generation and distant terrain (LOD) run on separate threads, so the game can't tell
whose data pack it is. The analyzer `tools/datapack_lag.py` does that (Python 3, no game needed):

```
python tools\datapack_lag.py --vanilla <path>\_vanilla-26.3 <server>\world\datapacks -o report.md
python tools\datapack_lag.py --each --vanilla <path>\_vanilla-26.3 <folder with packs> -o variants.md
```

It estimates how many noise samples each density function of the router costs (`final_density` per
block of new chunks, `chunk_surface_level` per column, including LOD), taking caches into account,
and compares it with vanilla. It shows the heaviest files, uncached functions used
several times, noises with many octaves, large surface rules, dense structure sets, features with
hundreds of attempts per chunk, carvers and broken JSON. `--each` compares pack variants that aren't
installed together. Example: `tools/example-report-26.3.md`.

The numbers are an estimate to show where to look. To confirm in game: `/at prof 60` while flying, or
`/pregen start 300` with and without the pack.

## Structures from data packs

`/at structures` finds the nearest structures of every data pack in the world (one per tick, so as not to
stall), `/at structures village` only those whose id contains `village`. `/at structlist`
shows which structures each data pack adds. The search uses vanilla `/locate` (radius 100 chunks).

## Other

`/at near [radius]`: entities around you; `/at clear <hostile|items|xp> [radius]`;
`/at stress <count> <mob>`; `/at mods`; `/at reload`.

## Configuration

`config/admintools.toml` (created automatically): permission level, profile duration, spike threshold,
mob sampling rate, how many lines to show.

## For other mods (shared core)

In your mod's `horizon-mixin.toml`:

```toml
[dependencies]
admintools = "^0.1"
```

and in the `[api]` code:

```rust
admintools::shared::has_flag("minecraft:overworld", pos, "no_pvp");
admintools::shared::natural_blocked(dim, "monster", pos);
let (cfg, problems) = admintools::shared::load_toml::<MyConfig>(&dir, "mymod.toml", DEFAULT);
```

## Files

- `admintools/`: the mod itself.
  - `api/src/core/`: rules without game types (config, regions, profiler, data packs).
  - `api/src/lib.rs`: glue to the Horizon hooks (commands, menu, spawning, profiler).
- `core-tests/`: `cargo test` checks the core without the game (18 tests).
- `tools/datapack_lag.py`: the data pack lag analyzer.
- `test-server/install.ps1`: the author's install script (copies the mod into the mods folder and rebuilds the engine; edit the paths at the top).

## Installing

Copy the `admintools` folder next to your other Horizon mods and rebuild the engine with them.
The mod is made for Horizon `fdc87ae` (nightly-2026-11-08).

## License

MIT, see [LICENSE](LICENSE). A mod for [Horizon](https://github.com/horizon-rust/horizon) (MIT, Ludovic Marie and the Horizon contributors).

Unofficial project, not affiliated with Mojang Studios or Microsoft. Minecraft is a trademark of Mojang Synergies AB.

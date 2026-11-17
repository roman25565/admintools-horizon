#!/usr/bin/env python3
"""Which datapack makes world generation (and the far terrain / LOD) slow, and what in it.

Reads datapacks (folders or .zip) without the game and estimates, per pack:

- terrain: how many noise samples each density function of the noise router costs per block
  (final_density, chunk_surface_level ...), with the caches (`interpolated`, `flat_cache`,
  `cache_2d`, `cache_once`) taken into account, compared with vanilla. The far terrain (LOD)
  samples the router's surface level and the material rule for every far column, so a heavy
  router or material rule slows LOD loading for every player;
- the heaviest functions by file, and functions used several times without a cache;
- noises with many octaves;
- material (surface) rule size and its noise conditions;
- structures: structure sets with small spacing, big jigsaws;
- features: placed features with many attempts per chunk; carvers with high probability.

Usage (any number of packs or datapacks folders, in load order; the last overrides):
    python datapack_lag.py F:\\MinecraftServers\\horizonTestServer\\world\\datapacks
    python datapack_lag.py --vanilla F:\\MinecraftTools\\DataPack\\_vanilla-26.3 pack1 pack2 -o report.md

Every number is an estimate from the JSON alone: use it to know where to look, then confirm with
/at prof in game (spikes while chunks generate) or by timing /pregen with and without the pack.
"""

from __future__ import annotations

import argparse
import json
import sys
import zipfile
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

# Registries we read: <ns>/worldgen/<kind>/<path>.json
KINDS = (
    "density_function", "noise", "noise_settings", "placed_feature", "configured_feature",
    "structure", "structure_set", "template_pool", "carver", "configured_carver", "biome",
    "material_rule",
)

# What one evaluation of a node costs besides its children, in "noise octave samples".
NODE_COST = {
    "minecraft:old_blended_noise": 16.0,
    "minecraft:end_islands": 4.0,
    "minecraft:weird_scaled_sampler": 0.5,
    "minecraft:spline": 0.2,
    "minecraft:beardifier": 1.0,
    "minecraft:blend_density": 0.1,
}
NOISE_NODES = {"minecraft:noise", "minecraft:shifted_noise", "minecraft:weird_scaled_sampler", "minecraft:shift_a", "minecraft:shift_b", "minecraft:shift"}
# How often a cached subtree is evaluated per block, roughly: a 4x8x4 interpolated cell is
# evaluated at its corners (~1/32 per block); a 2D cache once per column of ~384 blocks
# (counted as 1/48 with cell sharing); cache_once avoids repeats inside one evaluation.
CACHE_FACTOR = {
    "minecraft:interpolated": 1 / 32,
    "minecraft:flat_cache": 1 / 48,
    "minecraft:cache_2d": 1 / 48,
    "minecraft:cache_all_in_cell": 1 / 32,
    "minecraft:cache": 1 / 48,  # 26.3: generic cache, 2D when its input does not depend on y (see is_3d)
}
CACHES = set(CACHE_FACTOR) | {"minecraft:cache_once"}
Y_NODES = {"minecraft:y_clamped_gradient", "minecraft:gradient", "minecraft:old_blended_noise", "minecraft:interpolated",
           "minecraft:beardifier", "minecraft:blend_density", "minecraft:weird_scaled_sampler", "minecraft:end_islands"}


@dataclass
class Entry:
    pack: str
    path: str
    data: object


@dataclass
class World:
    """The merged registries of the given packs (last wins), and who defined what."""
    reg: dict = field(default_factory=lambda: {k: {} for k in KINDS})
    overrides: dict = field(default_factory=lambda: defaultdict(list))  # (kind,id) -> [packs]

    def add(self, pack: str, rel: str, raw: bytes):
        parts = rel.replace("\\", "/").split("/")
        if len(parts) < 5 or parts[0] != "data" or parts[2] != "worldgen" or not rel.endswith(".json"):
            return
        ns, kind = parts[1], parts[3]
        if kind not in self.reg:
            return
        ident = f"{ns}:{'/'.join(parts[4:])[:-5]}"
        try:
            data = json.loads(raw.decode("utf-8-sig"))
        except Exception as e:  # a broken file is itself worth reporting
            data = {"__error__": str(e)}
        self.reg[kind][ident] = Entry(pack, rel, data)
        self.overrides[(kind, ident)].append(pack)


def read_pack(world: World, path: Path):
    name = path.name
    if path.is_dir():
        for f in path.rglob("*.json"):
            rel = f.relative_to(path).as_posix()
            if rel.startswith("data/"):
                world.add(name, rel, f.read_bytes())
    elif path.suffix.lower() == ".zip":
        with zipfile.ZipFile(path) as z:
            for n in z.namelist():
                if n.startswith("data/") and n.endswith(".json"):
                    world.add(name, n, z.read(n))


def expand(paths):
    """A datapacks folder becomes its packs (sorted, as the game loads them); a pack stays."""
    out = []
    for p in paths:
        p = Path(p)
        if p.is_dir() and not (p / "pack.mcmeta").exists() and not (p / "data").exists():
            out += sorted(c for c in p.iterdir()
                          if (c.is_dir() and ((c / "pack.mcmeta").exists() or (c / "data").exists()))
                          or c.suffix.lower() == ".zip")
        else:
            out.append(p)
    return out


def octaves(noise) -> int:
    if not isinstance(noise, dict):
        return 1
    if "octave_count" in noise:
        mods = noise.get("amplitude_modifiers")
        if isinstance(mods, list) and mods:
            return sum(1 for a in mods if a) or 1
        return int(noise.get("octave_count") or 1)
    amps = noise.get("amplitudes")
    if isinstance(amps, list):
        return sum(1 for a in amps if a) or 1
    return 1


class Cost:
    """Evaluates the cost of density functions in one World."""

    def __init__(self, world: World):
        self.w = world
        self.memo: dict[str, tuple[float, float]] = {}
        self.uses: Counter = Counter()          # how often each named function is referenced
        self.uncached_uses: Counter = Counter()  # references not under any cache
        self.missing: set = set()
        self.stack: set = set()

    def noise_cost(self, ref) -> float:
        if isinstance(ref, dict):
            return float(octaves(ref)) * 2  # Perlin "double" noise samples two octave sets
        e = self.w.reg["noise"].get(ref)
        if e is None:
            self.missing.add(("noise", ref))
            return 2.0
        return float(octaves(e.data)) * 2

    def top_type(self, ident: str) -> str:
        e = self.w.reg["density_function"].get(ident)
        t = e.data.get("type", "") if e and isinstance(e.data, dict) else ""
        return t if ":" in t or not t else f"minecraft:{t}"

    def is_3d(self, n, seen=None) -> bool:
        seen = seen if seen is not None else set()
        if isinstance(n, str):
            ident = n if ":" in n else f"minecraft:{n}"
            if ident in seen:
                return False
            seen.add(ident)
            e = self.w.reg["density_function"].get(ident)
            return self.is_3d(e.data, seen) if e else False
        if isinstance(n, list):
            return any(self.is_3d(c, seen) for c in n)
        if not isinstance(n, dict):
            return False
        t = n.get("type", "")
        t = t if ":" in t else f"minecraft:{t}"
        if t in Y_NODES:
            return True
        if t in ("minecraft:noise", "minecraft:shifted_noise") and n.get("y_scale", 0) not in (0, 0.0):
            return True
        return any(self.is_3d(v, seen) for k, v in n.items() if k != "type")

    def named(self, ident: str, cached: bool) -> tuple[float, float]:
        self.uses[ident] += 1
        self_cached = self.top_type(ident) in CACHES
        if not cached and not self_cached:
            self.uncached_uses[ident] += 1
        if ident in self.memo:
            if self_cached and self.uses[ident] > 1:
                return (0.0, 0.0)  # same position: the cache answers
            return self.memo[ident]
        e = self.w.reg["density_function"].get(ident)
        if e is None and ident.endswith("/chunk_surface_level"):
            # 26.3 vanilla ships it built in; it is the preliminary surface level search.
            e = self.w.reg["density_function"].get(ident.replace("chunk_surface_level", "preliminary_surface_level"))
        if e is None:
            self.missing.add(("density_function", ident))
            return (1.0, 1.0)
        if ident in self.stack:
            return (0.0, 0.0)
        self.stack.add(ident)
        r = self.node(e.data, cached)
        self.stack.discard(ident)
        self.memo[ident] = r
        return r

    def node(self, n, cached: bool = False) -> tuple[float, float]:
        """(raw samples per evaluation, effective samples per block)."""
        if isinstance(n, (int, float)) or n is None:
            return (0.0, 0.0)
        if isinstance(n, str):
            return self.named(n if ":" in n else f"minecraft:{n}", cached)
        if not isinstance(n, dict):
            return (0.0, 0.0)
        t = n.get("type", "")
        t = t if ":" in t else f"minecraft:{t}"
        raw = NODE_COST.get(t, 0.05)
        if t in ("minecraft:noise", "minecraft:shifted_noise", "minecraft:weird_scaled_sampler"):
            raw += self.noise_cost(n.get("noise"))
        if t in ("minecraft:shift_a", "minecraft:shift_b", "minecraft:shift"):
            raw += self.noise_cost(n.get("argument") or n.get("noise"))
        factor = CACHE_FACTOR.get(t, 1.0)
        if t == "minecraft:cache" and self.is_3d(n.get("input")):
            factor = 1.0  # a 3D value is cached per block only (de-duplicates, does not skip blocks)
        sub_cached = cached or t in CACHES
        if t == "minecraft:find_top_surface":
            # Walks the column down cell by cell until the density turns solid: ~half the
            # world height / cell_height evaluations, once per column (this is what LOD pays).
            steps = max(1.0, 384 / max(1, n.get("cell_height", 8)) / 2)
            d = self.node(n.get("density"), sub_cached)
            return (raw + d[0] * steps, raw + d[0] * steps)
        if t == "minecraft:range_choice":
            i = self.node(n.get("input"), sub_cached)
            a = self.node(n.get("when_in_range"), sub_cached)
            b = self.node(n.get("when_out_of_range"), sub_cached)
            return (raw + i[0] + max(a[0], b[0]), raw + i[1] + max(a[1], b[1]))
        child_raw = child_eff = 0.0
        for k, v in n.items():
            if k in ("type", "noise", "argument") and not isinstance(v, dict):
                continue
            for c in (v if isinstance(v, list) else [v]):
                if isinstance(c, dict) and ("location" in c or "coordinate" in c or "points" in c):
                    r = self.spline(c, sub_cached)
                else:
                    r = self.node(c, sub_cached)
                child_raw += r[0]
                child_eff += r[1]
        total_raw = raw + child_raw
        eff = (raw + child_eff) * factor if factor != 1.0 else raw + child_eff
        return (total_raw, eff)

    def spline(self, s, cached) -> tuple[float, float]:
        raw = eff = 0.0
        if isinstance(s, dict):
            for k in ("coordinate", "value"):
                if k in s:
                    r = self.node(s[k], cached) if not (isinstance(s[k], dict) and "points" in s[k]) else self.spline(s[k], cached)
                    raw += r[0]
                    eff += r[1]
            for p in s.get("points", []) or []:
                r = self.spline(p, cached)
                raw += r[0]
                eff += r[1]
            if "spline" in s:
                r = self.spline(s["spline"], cached)
                raw += r[0]
                eff += r[1]
        elif isinstance(s, (str, dict)):
            return self.node(s, cached)
        return (raw + 0.05, eff + 0.05)


def rule_stats(rule) -> tuple[int, int, int]:
    """(nodes, depth, noise conditions) of a material/surface rule."""
    if isinstance(rule, list):
        rs = [rule_stats(r) for r in rule]
        return (sum(r[0] for r in rs), max((r[1] for r in rs), default=0), sum(r[2] for r in rs))
    if not isinstance(rule, dict):
        return (1, 1, 0)
    nodes, depth, noises = 1, 0, 0
    t = str(rule.get("type", ""))
    if "noise" in t:
        noises += 1
    for v in rule.values():
        if isinstance(v, (dict, list)):
            a, b, c = rule_stats(v)
            nodes += a
            depth = max(depth, b)
            noises += c
    return (nodes, depth + 1, noises)


def placement_attempts(pf) -> float:
    """Rough feature attempts per chunk from a placed feature's placement modifiers."""
    n = 1.0
    for m in (pf.get("placement") or []) if isinstance(pf, dict) else []:
        if not isinstance(m, dict):
            continue
        t = str(m.get("type", ""))
        if t.endswith("count") or t.endswith("count_on_every_layer"):
            c = m.get("count", 1)
            if isinstance(c, dict):
                c = c.get("max_inclusive", c.get("value", {}).get("max_inclusive", 4) if isinstance(c.get("value"), dict) else 4)
            try:
                n *= max(float(c), 0.0)
            except (TypeError, ValueError):
                n *= 4
        elif t.endswith("rarity_filter"):
            try:
                n /= max(float(m.get("chance", 1)), 1.0)
            except (TypeError, ValueError):
                pass
    return n


@dataclass
class Finding:
    pack: str
    weight: float  # how much it likely costs; findings are sorted by it
    what: str
    where: str


def analyse(world: World, vanilla: World | None):
    findings: list[Finding] = []
    cost = Cost(world)
    vcost = Cost(vanilla) if vanilla else None
    router_rows = []
    for ident, e in world.reg["noise_settings"].items():
        d = e.data if isinstance(e.data, dict) else {}
        router = d.get("noise_router", {}) or {}
        for key, fn in router.items():
            raw, eff = cost.node(fn)
            vr = None
            if vcost and ident in vanilla.reg["noise_settings"]:
                vd = vanilla.reg["noise_settings"][ident].data.get("noise_router", {})
                if key in vd:
                    vr = vcost.node(vd[key])
            router_rows.append((ident, key, raw, eff, vr, e.pack))
            ratio = (eff / vr[1]) if vr and vr[1] > 0 else None
            lod = key in ("chunk_surface_level", "preliminary_surface_level", "initial_density_without_jaggedness")
            unit = "на стовпець" if lod else "на блок"
            if ratio and ratio > 1.5:
                findings.append(Finding(e.pack, (ratio - 1) * 10 * (3 if lod else 1), f"{ident} router `{key}`: ~{eff:.1f} семплів шуму {unit} проти ~{vr[1]:.1f} у ванілі (x{ratio:.1f})" + (" — це рахує далекий ландшафт (LOD) для кожного стовпця" if lod else ""), e.path))
            elif not vr and eff > 20:
                findings.append(Finding(e.pack, eff * (3 if lod else 1), f"{ident} router `{key}`: ~{eff:.1f} семплів шуму на блок (сирих {raw:.0f} за обчислення)", e.path))
        mr = d.get("material_rule") or d.get("surface_rule")
        if mr is not None:
            nodes, depth, noises = rule_stats(mr)
            if nodes > 400 or noises > 20:
                findings.append(Finding(e.pack, nodes / 20 + noises, f"{ident}: правило поверхні/матеріалів велике ({nodes} вузлів, глибина {depth}, {noises} умов із шумом) — рахується для кожного стовпця, зокрема LOD", e.path))
    # Heaviest named functions and uncached repeats.
    for ident, (raw, eff) in sorted(cost.memo.items(), key=lambda kv: -kv[1][1])[:12]:
        e = world.reg["density_function"].get(ident)
        if e and eff > 8:
            findings.append(Finding(e.pack, eff / 2, f"важка функція `{ident}`: ~{eff:.1f} семплів на блок (сирих {raw:.0f})", e.path))
    for ident, n in cost.uncached_uses.items():
        e = world.reg["density_function"].get(ident)
        raw = cost.memo.get(ident, (0, 0))[0]
        if e and n >= 2 and raw >= 6:
            findings.append(Finding(e.pack, raw * (n - 1) / 4, f"`{ident}` (сирих {raw:.0f}) використовується {n} разів без кешу: загорни у `minecraft:cache` (26.3) або flat_cache/cache_once (старі версії)", e.path))
    for ident, e in world.reg["noise"].items():
        o = octaves(e.data)
        vo = octaves(vanilla.reg["noise"][ident].data) if vanilla and ident in vanilla.reg["noise"] else None
        if (vo is not None and o > vo) or (vo is None and o >= 10):
            findings.append(Finding(e.pack, o / 2, f"шум `{ident}` має {o} октав" + (f" (ваніла {vo})" if vo is not None else ""), e.path))
    # Structures.
    for ident, e in world.reg["structure_set"].items():
        d = e.data if isinstance(e.data, dict) else {}
        pl = d.get("placement", {}) or {}
        spacing = pl.get("spacing")
        structs = d.get("structures", []) or []
        if isinstance(spacing, (int, float)) and spacing > 0:
            per_k = 1000 / (spacing * spacing)
            if spacing < 12 or len(structs) > 40:
                findings.append(Finding(e.pack, per_k * 2 + len(structs) / 20, f"набір структур `{ident}`: spacing {spacing}, {len(structs)} структур, ~{per_k:.1f} спроб на 1000 чанків (кожна перевіряє біом і висоту)", e.path))
    for ident, e in world.reg["structure"].items():
        d = e.data if isinstance(e.data, dict) else {}
        size = d.get("size") or d.get("max_depth")
        dist = d.get("max_distance_from_center")
        if isinstance(size, int) and size >= 8 or isinstance(dist, int) and dist > 116:
            findings.append(Finding(e.pack, (size or 0) + (dist or 0) / 32, f"велика jigsaw-структура `{ident}`: size {size}, max_distance_from_center {dist}", e.path))
    for ident, e in world.reg["placed_feature"].items():
        n = placement_attempts(e.data)
        if n >= 64:
            findings.append(Finding(e.pack, n / 16, f"фіча `{ident}`: ~{n:.0f} спроб на чанк", e.path))
    for kind in ("configured_carver", "carver"):
        for ident, e in world.reg[kind].items():
            p = (e.data.get("config", {}) or {}).get("probability") if isinstance(e.data, dict) else None
            if isinstance(p, (int, float)) and p > 0.2:
                findings.append(Finding(e.pack, p * 20, f"карвер `{ident}`: probability {p} (ваніла ~0.08-0.15)", e.path))
    for kind, reg in world.reg.items():
        for ident, e in reg.items():
            if isinstance(e.data, dict) and "__error__" in e.data:
                findings.append(Finding(e.pack, 5, f"зламаний JSON `{ident}`: {e.data['__error__']}", e.path))
    return findings, router_rows, cost


def report(packs, findings, router_rows, cost, world) -> str:
    out = ["# Датапаки і лаги генерації\n"]
    out.append("Оцінка з JSON, без гри: де шукати. Перевір у грі: `/at prof 60` під час польоту в нові землі.\n")
    by_pack = defaultdict(float)
    for f in findings:
        by_pack[f.pack] += f.weight
    out.append("## Рейтинг паків (більше = важче)\n\n| пак | бал | що саме |\n|---|---|---|")
    for p in sorted((x.name for x in packs), key=lambda n: -by_pack.get(n, 0)):
        top = sorted((f for f in findings if f.pack == p), key=lambda f: -f.weight)[:2]
        out.append(f"| {p} | {by_pack.get(p, 0):.0f} | {'; '.join(f.what for f in top) or 'нічого помітного'} |")
    out.append("\n## Роутер шуму (ландшафт і LOD)\n\n| noise_settings | ключ | сирих семплів | на блок | ваніла на блок | пак |\n|---|---|---|---|---|---|")
    for ident, key, raw, eff, vr, pack in router_rows:
        out.append(f"| {ident} | {key} | {raw:.0f} | {eff:.1f} | {'' if not vr else f'{vr[1]:.1f}'} | {pack} |")
    out.append("\n## Знахідки (найважчі першими)\n")
    for f in sorted(findings, key=lambda f: -f.weight):
        out.append(f"- **{f.pack}**: {f.what}  \n  `{f.where}`")
    overridden = [(k, v) for k, v in world.overrides.items() if len(v) > 1]
    if overridden:
        out.append("\n## Один файл з кількох паків (перемагає останній)\n")
        for (kind, ident), ps in sorted(overridden)[:60]:
            out.append(f"- {kind} `{ident}`: {' → '.join(ps)}")
    if cost.missing:
        out.append(f"\n_Не знайдено {len(cost.missing)} посилань (ванільні файли не передані через --vanilla): оцінка для них умовна._")
    return "\n".join(out) + "\n"


def compare_each(packs, vanilla_path) -> str:
    """Each pack alone over vanilla: which variant is the heaviest and why."""
    vanilla = None
    if vanilla_path:
        vanilla = World()
        read_pack(vanilla, Path(vanilla_path))
    rows = []
    for p in packs:
        world = World()
        if vanilla_path:
            read_pack(world, Path(vanilla_path))
        read_pack(world, p)
        findings, router_rows, _ = analyse(world, vanilla)
        findings = [f for f in findings if f.pack == p.name]
        fd = {(i, k): (eff, vr) for i, k, raw, eff, vr, pk in router_rows}
        rows.append((p.name, sum(f.weight for f in findings), fd.get(("minecraft:overworld", "final_density")),
                     fd.get(("minecraft:overworld", "chunk_surface_level")) or fd.get(("minecraft:overworld", "preliminary_surface_level")), sorted(findings, key=lambda f: -f.weight)))
    rows.sort(key=lambda r: -r[1])
    out = ["# Датапаки і лаги генерації: кожен пак окремо поверх ванілі\n",
           "Оцінка з JSON, без гри. `final_density` рахується для кожного блоку нових чанків; "
           "`chunk_surface_level` і правило поверхні рахує й далекий ландшафт (LOD). "
           "Перевір у грі: `/at prof 60` під час польоту в нові землі.\n",
           "| пак | бал | final_density, семплів/блок | пошук поверхні (LOD), семплів/стовпець | головне |", "|---|---|---|---|---|"]
    def cell(v):
        if not v:
            return "ваніла"
        eff, vr = v
        return f"{eff:.1f}" + (f" (x{eff / vr[1]:.1f})" if vr and vr[1] > 0 else "")
    for name, score, fd, psl, fs in rows:
        out.append(f"| {name} | {score:.0f} | {cell(fd)} | {cell(psl)} | {fs[0].what if fs else 'нічого помітного'} |")
    for name, score, fd, psl, fs in rows:
        if not fs:
            continue
        out.append(f"\n## {name}\n")
        for f in fs[:10]:
            out.append(f"- {f.what}  \n  `{f.where}`")
    return "\n".join(out) + "\n"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("packs", nargs="+", help="datapack folders/zips, or a datapacks folder")
    ap.add_argument("--vanilla", help="vanilla worldgen extracted as a pack (data/minecraft/worldgen/...)")
    ap.add_argument("--each", action="store_true",
                    help="analyse every pack alone on top of vanilla and compare them (for variants that are never loaded together)")
    ap.add_argument("-o", "--out", help="write the Markdown report here (default: print)")
    a = ap.parse_args(argv)
    packs = expand(a.packs)
    if a.vanilla:
        vp = Path(a.vanilla).resolve()
        packs = [p for p in packs if p.resolve() != vp]
    if a.each:
        text = compare_each(packs, a.vanilla)
        if a.out:
            Path(a.out).write_text(text, encoding="utf-8")
            print(f"written {a.out}")
        else:
            sys.stdout.reconfigure(encoding="utf-8")
            print(text)
        return
    world = World()
    vanilla = None
    if a.vanilla:
        vanilla = World()
        read_pack(vanilla, Path(a.vanilla))
        read_pack(world, Path(a.vanilla))  # the packs override vanilla, as in the game
    for p in packs:
        read_pack(world, p)
    findings, rows, cost = analyse(world, vanilla)
    findings = [f for f in findings if not a.vanilla or f.pack != Path(a.vanilla).name]
    text = report(packs, findings, rows, cost, world)
    if a.out:
        Path(a.out).write_text(text, encoding="utf-8")
        print(f"written {a.out}")
    else:
        sys.stdout.reconfigure(encoding="utf-8")
        print(text)


if __name__ == "__main__":
    main()

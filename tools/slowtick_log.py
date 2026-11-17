#!/usr/bin/env python3
"""Підсумок рядків "Horizon: slow tick" з логу сервера: що саме їсть тік.

python slowtick_log.py logs/latest.log [--budget 33]
Рахує для кожного кроку суму мс, у скількох повільних тіках він був
найбільшим (крім обгорток level_*/tick), і групує тіки за кількістю мобів.
"""
import re, sys, argparse, collections

WRAP = re.compile(r"^(level_|tick$|tick_lan$|lan_tick_level$|tick_mobs$|spawn_natural$)")
LINE = re.compile(r"slow tick ([\d.]+) ms, cpu ([\d.]+) ms \((\d+) mobs\): (.*?)(?:; slowest: (.*))?$")
SPAN = re.compile(r"([A-Za-z_][\w ]*?) ([\d.]+)(?:/([\d.]+))?$")

GROUP = {
    "streaming": "генерація/завантаження чанків", "postprocess_chunks": "генерація/завантаження чанків",
    "lan_chunks": "відправка чанків гравцям", "tick_lan_packets": "пакети гравців", "lan_events": "пакети гравців",
    "sn_attempts": "природний спавн", "sn_candidates": "природний спавн", "sn_materialize": "природний спавн",
    "creature_loop": "тік мобів", "tick_creature_entities": "тік мобів", "creature_prepare": "тік мобів",
    "random_ticks": "random tick блоків", "tb_scheduled": "заплановані тіки блоків",
    "unload_entities": "вивантаження сутностей", "tick_block_entities": "блок-сутності",
    "mods_server_tick_end": "моди", "mods_server_tick_start": "моди",
}

def main():
    ap = argparse.ArgumentParser(); ap.add_argument("log"); ap.add_argument("--budget", type=float, default=33.3)
    a = ap.parse_args()
    total = collections.Counter(); top = collections.Counter(); n = 0; wall = 0.0
    buckets = collections.defaultdict(list); huge = []
    for raw in open(a.log, encoding="utf-8", errors="replace"):
        m = LINE.search(raw)
        if not m: continue
        w, cpu, mobs, spans = float(m[1]), float(m[2]), int(m[3]), m[4]
        n += 1; wall += w
        parsed = []
        for part in spans.split(", "):
            s = SPAN.match(part.strip())
            if s: parsed.append((s[1], float(s[2])))
        for name, ms in parsed: total[name] += ms
        leaf = [(ms, nm) for nm, ms in parsed if not WRAP.match(nm)]
        if leaf: top[max(leaf)[1]] += 1
        buckets[min(mobs // 200 * 200, 1000)].append(w)
        if w >= 500: huge.append((w, mobs, ", ".join(f"{nm} {ms:g}" for nm, ms in parsed[:4])))
    if not n: print("Немає рядків slow tick"); return
    print(f"# Повільні тіки: {n}, разом {wall/1000:.1f} с понад норму {a.budget:g} мс\n")
    print("## Що найчастіше найбільше в повільному тіку\n")
    for nm, c in top.most_common(12):
        print(f"- {nm} ({GROUP.get(nm, '?')}): {c} тіків ({100*c/n:.0f}%), разом {total[nm]/1000:.1f} с")
    g = collections.Counter()
    for nm, ms in total.items():
        if nm in GROUP and not WRAP.match(nm): g[GROUP[nm]] += ms
    print("\n## За групами (с)\n")
    for k, v in g.most_common(): print(f"- {k}: {v/1000:.1f}")
    print("\n## Середній повільний тік за кількістю мобів\n")
    for b in sorted(buckets):
        v = buckets[b]; print(f"- {b}+ мобів: {len(v)} тіків, середньо {sum(v)/len(v):.0f} мс")
    if huge:
        print("\n## Зависання від 500 мс\n")
        for w, mobs, s in sorted(huge, reverse=True)[:10]: print(f"- {w:.0f} мс, {mobs} мобів: {s}")

if __name__ == "__main__": main()

# Датапаки і лаги генерації: кожен пак окремо поверх ванілі

Оцінка з JSON, без гри. `final_density` рахується для кожного блоку нових чанків; `chunk_surface_level` і правило поверхні рахує й далекий ландшафт (LOD). Перевір у грі: `/at prof 60` під час польоту в нові землі.

| пак | бал | final_density, семплів/блок | пошук поверхні (LOD), семплів/стовпець | головне |
|---|---|---|---|---|
| epicterrain-26.3 | 569 | 24.8 (x9.5) | 15148.8 (x5.3) | minecraft:overworld router `chunk_surface_level`: ~15148.8 семплів шуму на стовпець проти ~2883.7 у ванілі (x5.3) — це рахує далекий ландшафт (LOD) для кожного стовпця |
| CustomWorldgen-26.3-gentle | 2 | 1.2 (x0.5) | 0.1 (x0.0) | шум `minecraft:erosion` має 5 октав (ваніла 4) |
| CustomWorldgen-26.3-gentle.zip | 2 | 1.2 (x0.5) | 0.1 (x0.0) | шум `minecraft:erosion` має 5 октав (ваніла 4) |
| CustomWorldgen-26.3-simple | 2 | 1.2 (x0.5) | 0.1 (x0.0) | шум `minecraft:erosion` має 5 октав (ваніла 4) |
| CustomWorldgen-26.3 | 0 | 2.7 (x1.0) | 2984.4 (x1.0) | нічого помітного |
| CustomWorldgen-26.3-lite | 0 | 1.5 (x0.6) | 2984.4 (x1.0) | нічого помітного |
| CustomWorldgen-26.3-low | 0 | 1.1 (x0.4) | 0.3 (x0.0) | нічого помітного |
| CustomWorldgen-26.3-vanilla-terrain | 0 | 2.7 (x1.0) | 2883.7 (x1.0) | нічого помітного |

## epicterrain-26.3

- minecraft:overworld router `chunk_surface_level`: ~15148.8 семплів шуму на стовпець проти ~2883.7 у ванілі (x5.3) — це рахує далекий ландшафт (LOD) для кожного стовпця  
  `data/minecraft/worldgen/noise_settings/overworld.json`
- minecraft:overworld router `depth`: ~12.1 семплів шуму на блок проти ~1.1 у ванілі (x11.0)  
  `data/minecraft/worldgen/noise_settings/overworld.json`
- minecraft:overworld router `final_density`: ~24.8 семплів шуму на блок проти ~2.6 у ванілі (x9.5)  
  `data/minecraft/worldgen/noise_settings/overworld.json`
- minecraft:overworld router `temperature`: ~6.3 семплів шуму на блок проти ~1.0 у ванілі (x6.3)  
  `data/minecraft/worldgen/noise_settings/overworld.json`
- minecraft:overworld router `vegetation`: ~6.3 семплів шуму на блок проти ~1.0 у ванілі (x6.3)  
  `data/minecraft/worldgen/noise_settings/overworld.json`
- важка функція `minecraft:overworld/caves/entrances`: ~40.4 семплів на блок (сирих 244)  
  `data/minecraft/worldgen/density_function/overworld/caves/entrances.json`
- `minecraft:overworld/depth` (сирих 76) використовується 2 разів без кешу: загорни у `minecraft:cache` (26.3) або flat_cache/cache_once (старі версії)  
  `data/minecraft/worldgen/density_function/overworld/depth.json`
- `etn:overworld/depth` (сирих 76) використовується 2 разів без кешу: загорни у `minecraft:cache` (26.3) або flat_cache/cache_once (старі версії)  
  `data/etn/worldgen/density_function/overworld/depth.json`
- важка функція `minecraft:overworld/caves/entrances1`: ~37.9 семплів на блок (сирих 167)  
  `data/minecraft/worldgen/density_function/overworld/caves/entrances1.json`
- важка функція `minecraft:overworld/final_density`: ~24.8 семплів на блок (сирих 585)  
  `data/minecraft/worldgen/density_function/overworld/final_density.json`

## CustomWorldgen-26.3-gentle

- шум `minecraft:erosion` має 5 октав (ваніла 4)  
  `data/minecraft/worldgen/noise/erosion.json`

## CustomWorldgen-26.3-gentle.zip

- шум `minecraft:erosion` має 5 октав (ваніла 4)  
  `data/minecraft/worldgen/noise/erosion.json`

## CustomWorldgen-26.3-simple

- шум `minecraft:erosion` має 5 октав (ваніла 4)  
  `data/minecraft/worldgen/noise/erosion.json`

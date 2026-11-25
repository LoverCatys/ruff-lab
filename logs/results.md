# Test Results Log

System: **AMD Radeon RX 6600 XT 8 GB** (Adrenalin `26.11.2` / `32.0.21045.29003`), **Ryzen 5 3600**, **16 GB RAM**, **Windows 11 (`26200`)**, **5400 RPM SATA HDD**.

| # | Date | Build / Commit | Mode & Flags | Stage Reached | Duration | FPS (Avg / Peak) | Notes |
|---|---|---|---|---|---|---|---|
| 01 | 2026-11-25 | `klbf-lab-beta` Lab 39 (`0ad4037`) | `series-s` + FSR, `LATE_WAIT=1`, fresh cache (no pre-build) | First-boot setup → World load → `Game Running` (Mission `0x98c85f2a`, frame 15520) | 12m 40s | 30.0 fps (menus/load) | Passed setup & HUD cleanly in Series S mode; zero `amdxc64.dll` crashes; hit 30 s RAGE GPU watchdog (`+0x23cf683 → +0x2215c35`) on first world frame compiling hundreds of shaders on the fly |
| 02 | 2026-11-25 | `klbf-lab-beta` Lab 39 (`0ad4037`) | `series-s` + FSR, `LATE_WAIT=1`, partial shader cache (no `host-pipelines.bin`) | Character loading screen → `Game Running` (Mission `0x98c85f2a`, `TileHeap 2090/3977 MB`, RAM `5.88 GB` / commit `8.66 GB`) | 14m 39s | 30.0 fps (menus/load) | Same 30 s RAGE GPU watchdog on first world frame caused by `VICTORY_PIPELINE_GATE_S` waiting on unbuilt pipelines; offline pre-build (`translate --cache --pipelines`) started — reached ~85% (~23k / 27,144) with **0 driver crashes** |

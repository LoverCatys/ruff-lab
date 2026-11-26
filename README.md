<h1 align="center">ruff-lab</h1>

<p align="center">
  <b>An open-source Xbox Series X|S (Scarlett) compatibility runtime fork tuned for mid-range PCs: 8 GB VRAM, 16 GB RAM, and rotational HDDs.</b>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/base-KLBF%20Lab%2039-1f46c8" alt="Base: KLBF Lab 39">
  <img src="https://img.shields.io/badge/GPU-AMD%20RX%206600%20XT%208GB-ed1c24" alt="GPU: AMD RX 6600 XT 8GB">
  <img src="https://img.shields.io/badge/RAM-16%20GB-1f46c8" alt="RAM: 16 GB">
  <img src="https://img.shields.io/badge/license-GPL--3.0-555" alt="GPL-3.0">
</p>

> [!NOTE]
> **No game files, dumps, or pre-built binaries are included in this repository.** You must supply your own legally obtained files and build from source.

---

## Status (2026-11-25)

**Stage 6 — In-game (player-controlled gameplay verified)** on an 8 GB VRAM / 16 GB RAM PC.

Tested on: **Windows 11 x64 (`26200`)** · **AMD Radeon RX 6600 XT 8 GB** (RDNA2, Adrenalin **26.11.2** / `32.0.21045.29003`) · **AMD Ryzen 5 3600** (6C/12T) · **16 GB RAM** (pagefile on SATA SSD `F:`) · **500 GB 5400 RPM SATA HDD** (game files on `D:`).

| Lane / Stage | Status | Details |
|---|---|---|
| **Boot & D3D12 init** | ✅ Works | Native Wave64 (`min=32 max=64 compute=64`); `0.5–1.2 ms` GPU |
| **Setup, menus & pause UI (`series-s` + FSR)** | ✅ Works | **30.0 fps** in menus/loading, **22 fps** in pause menu (`VI` HUD, map/game/settings tabs, clean blur) |
| **Offline pipeline pre-build** | ✅ Works | `27,144 / 27,144` pipelines built in `1,029 s` with **0 `amdxc64.dll` crashes** |
| **Player control (Stage 6)** | ✅ Verified | **Native DualShock 4 (`0x09CC`) HID input** — walked & rotated camera (`L`/`R`) through the prison yard and cell, interacted with cellmate (`LT`), followed the guard through the corridor & dayroom, progressed through the counselor's office to the bank robbery sequence (with audio) |
| **Picture (in-world)** | 🟢 Natural lighting (no magenta) | `ambient.rs` (`level=0.6`, `reflections held=0.5`) eliminates AMD magenta RTGI & whitewashed reflections: sunlit exterior with Lucia & parking lot, 20+ prison yard inmates, prison cell interior, volumetric hallway lighting, dayroom with live TV screen, counselor's office, and bank robbery with hostages & cash; some HDD LOD streaming lag & occasional dark face lighting in shadow |
| **Speed (in-world)** | 🟡 **3.62 fps avg / 3.70 median / 7.30 peak** | **250 pure 3D samples** (`draws >= 300`, avg `3,037` / peak `8,357 draws/frame`), 720p upscaled to 1080p via AMD FidelityFX SDK v2.4.0 (`--console series-s`); `30.0 fps` in menus |
| **Stability** | ✅ **39.1 min** without a crash | Clean user exit, `0` unhandled exceptions, `0` device removals, `0` `amdxc64.dll` crashes; adaptive `reserve_mb=1024` on `budget_mb=7379` active |

Full session-by-session logs: [logs/results.md](logs/results.md).

---

## What `ruff-lab` adds on top of KLBF Lab 39

Most compatibility forks are tested on 32 GB RAM and 12–16 GB VRAM cards. `ruff-lab` focuses on making the runtime viable on **8 GB GPUs, 16 GB RAM hosts, and HDD storage**:

| Area | Problem on 8 GB / 16 GB / HDD | Fix in `ruff-lab` |
|---|---|---|
| **8 GB VRAM texture starvation** | `targets_room_kept()` hardcoded a `3,072 MiB` reserve (measured on 16 GB Series X runs), leaving only ~2.5 GB for textures on 8 GB cards (`no room for new ones so_far=25500`), and refusing textures at 90% VRAM left ground, hands, and character skin pitch black | Adaptive `default_targets_room_mb()` scales the reserve to 14% / max `1,024 MiB` in Series S 720p mode and 22% / max `1,536 MiB` on ≤ 9 GB GPUs (+1.5–2.0 GB VRAM reclaimed), on-demand `resource::let_go_idle()`, plus mip-dropped short-room fallback (`VICTORY_TEXTURE_SHORT`, default `256` px up to 97% VRAM, & optional `VICTORY_TEXTURE_DROP_MIPS`, adapted from [ka1qux/BRVIctory-ka1qux](https://github.com/ka1qux/BRVIctory-ka1qux)) so textures load at ~87 KB instead of turning black |
| **Magenta / pitch-black AMD lighting** | Without ray-traced GI, `CS_ScreenProbes_TemporalResolve` left bright magenta on foliage/skin/cloth and pitch-black shadows; unoccluded reflections washed out interiors | Ported `ambient.rs` from [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) (`crates/victory-gpu/src/ambient.rs`): replaces broken RTGI resolve with warm ambient light + depth-based ambient occlusion (`VICTORY_AMBIENT`, default `0.6`, `off` to disable) and clamps unoccluded reflections (`VICTORY_AMBIENT_REFLECT`, default `0.5`) |
| **16 GB RAM staging bloat** | Every sampled texture kept a full-size staging copy in system RAM, pushing commit to 12–20 GB and thrashing pagefiles | Integrated [PastiiJay/klbf-lab#23](https://github.com/PastiiJay/klbf-lab/pull/23) (by `spy0eye-beep`): releases staging buffers of unchanged textures after 2 s / 30 frames while keeping metadata |
| **6-core CPU descriptor lock contention** | Global mutex on `HostHeap::mirror` stalled multi-threaded descriptor updates | Integrated [PastiiJay/klbf-lab#16](https://github.com/PastiiJay/klbf-lab/pull/16) (by `Vladimir-Human`): lock-free Acquire/Release fast path + 256 cache-aligned stripes |
| **Rotational HDD seek thrashing** | DirectStorage spawned `available_parallelism()` (12) concurrent file reader threads | Added `VICTORY_DSTORAGE_WORKERS` (`1..=64`, default = CPU count) in `crates/victory-hle/src/modules/dstorage.rs` so HDD users can set `2`–`4` workers |
| **Native Sony DS4 / DualSense input** | DualShock 4 & DualSense controllers required `x360ce` / `ViGEmBus` / `DS4Windows`, which often broke or failed to bind to `GameInput` | Native Windows HID reader (`crates/victory-hle/src/modules/gameinput/sony.rs`, adapted from [realblastex/flamingo](https://github.com/realblastex/flamingo)) supporting **DualShock 4** (v1/v2/dongle) & **DualSense / Edge** over USB and Bluetooth as slot 0 fallback (`VICTORY_SONY_PAD=0` to disable) |
| **BCPack fragment edge cases** | Corner-case single-fragment / zero-count BCPack textures could fail decompression | Backported [Marvisak/VIctory#2648](https://github.com/Marvisak/VIctory/pull/2648) & [#2649](https://github.com/Marvisak/VIctory/pull/2649) in `crates/victory-formats/src/bcpack.rs` |
| **Release binary CPU throughput** | Default release profile compiled without LTO across crates | Enabled `lto = "thin"` and `codegen-units = 1` in `Cargo.toml` (from [Belobug/VIctory-Belobug-AMD#96](https://github.com/Belobug/VIctory-Belobug-AMD/pull/96)) |
| **Refused AMD shaders & pipelines** | 40+ shaders failed translation on uniform static-sampler branches or split wave64 lane masks; D3D12 rejected pipelines with blending enabled on integer render targets | Ported static-sampler branch selection, integer-target blend bypass (`ddb73326b0`), and split wave64 lane-mask recovery (`76749ce58d`) from [DexNom/VIctory](https://github.com/DexNom/VIctory), plus 3D `DispatchMesh(x, y, z)` forwarding (`VIctory#2624`) |
| **16 GB RAM commit pressure & pipeline worker bloat** | Background pipeline workers spawned 6+ threads (~3–5 GB extra RAM) and GPU textures could exhaust Windows commit (`error 1455`) | Added `VICTORY_PIPELINE_WORKERS` ([Marvisak/VIctory#2752](https://github.com/Marvisak/VIctory/pull/2752)) and `VICTORY_COMMIT_RESERVE_MB` (adapted from [ka1qux/BRVIctory-ka1qux](https://github.com/ka1qux/BRVIctory-ka1qux) `8cac16489a`) to protect free system commit |
| **CPU frame time: copies, view guards, remaps & BC7 decode** | `ReadProcessMemory` kernel crossings on hot host copies (`37–52 ms/frame`), per-block `BTreeSet` clean-block scans, repeated `VirtualQuery` calls on unmapped blocks, redundant page remap splits/commits, and slower BC7 BCPack decoding | Ported in-process `rep movsb` + VEH fault-safe host copies (`plaincopy.rs`, `VICTORY_COPY_PLAIN`, from [fablinoo/kelvaroq](https://github.com/fablinoo/kelvaroq) `0044-copy-plain.patch`), interval `CleanBlocks` + per-frame `Uncommitted` query cache (`VICTORY_UNCOMMITTED_MEMO`, from [edsuuu/VI-BR-PORT](https://github.com/edsuuu/VI-BR-PORT) `ac6e475108` & `136efde0ab`), lean page remaps (`VICTORY_REMAP_LEAN=1`, from `ka1qux` `b1d7012a44` / `victory-fork#1069`), and +27% BC7 BCPack lookup tables ([Marvisak/VIctory#2735](https://github.com/Marvisak/VIctory/pull/2735)) |

---

## Building & Running

Requires Windows 10/11 x64, AVX2 CPU, D3D12 SM 6.6 + Mesh Shaders GPU, Rust (`1.98.1`, pinned in `rust-toolchain.toml`), MSVC C++ Build Tools, and [AMD FidelityFX SDK v2.4.0](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/releases) for Series S + FSR mode.

```powershell
copy victory.example.toml victory.toml   # set game_dir outside the repo
cargo test -p victory-gpu --lib -- --test-threads=1
cargo test -p victory-hle --lib dstorage
cargo build --release -p victory
```

Pre-build pipelines once (recommended on AMD):

```powershell
$env:VICTORY_SHADER_OPT = '0'
$env:VICTORY_SKIP_PIPELINES = (Get-Content docs\klbf\skip-amd.txt -Raw).Trim()
.\target\release\victory.exe translate --cache --pipelines --jobs 8 "<game_dir>\shaders.rpf"
```

Run (`--console series-s` + AMD FSR; add `$env:VICTORY_DSTORAGE_WORKERS = '3'` on HDD):

```powershell
$env:VICTORY_SHADER_OPT = '0'
$env:VICTORY_SKIP_PIPELINES = (Get-Content docs\klbf\skip-amd.txt -Raw).Trim()
$env:VICTORY_LATE_WAIT = '1'
$env:VICTORY_WIDE_BACK = '1'
$env:VICTORY_PIPELINE_WAIT_MS = '24'
$env:VICTORY_RELAY_COPY_QUEUE = '0'
$env:VICTORY_DSTORAGE_WORKERS = '3'
$env:VICTORY_FFX_DIR = '<FidelityFX SDK>\Kits\FidelityFX\signedbin'
.\target\release\victory.exe run --console series-s "<game_dir>\game_scarlett_master.exe" --execute
```

---

## Credits & License

- [Marvisak/VIctory](https://github.com/Marvisak/VIctory) — original Xbox Series X|S compatibility runtime for Windows.
- [fxceless/victory-fork](https://github.com/fxceless/victory-fork) — renderer and D3D12 work.
- [PastiiJay/klbf-lab](https://github.com/PastiiJay/klbf-lab) / [klbf-lab-beta](https://github.com/PastiiJay/klbf-lab-beta) — Lab 39 AMD fixes, Series S HUD, water physics, and FSR frame generation.
- [realblastex/flamingo](https://github.com/realblastex/flamingo) — native DualSense HID reader design adapted for DS4 + DualSense in `gameinput/sony.rs`.
- [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) — ambient light & reflection clamp pass (`ambient.rs`) fixing AMD magenta RTGI, and release LTO optimization (`#96`).
- [DexNom/VIctory](https://github.com/DexNom/VIctory) — uniform static-sampler branch lowering, integer-target blend bypass (`ddb73326b0`), and split wave64 lane-mask recovery (`76749ce58d`).
- [ka1qux/BRVIctory-ka1qux](https://github.com/ka1qux/BRVIctory-ka1qux) — short-room mip-dropped textures (`VICTORY_TEXTURE_SHORT` / `VICTORY_TEXTURE_DROP_MIPS`), Windows commit reserve (`VICTORY_COMMIT_RESERVE_MB`), lean page remaps (`VICTORY_REMAP_LEAN=1`, after `victory-fork#1069`), and 3D `DispatchMesh` forwarding (`VIctory#2624`).
- [fablinoo/kelvaroq](https://github.com/fablinoo/kelvaroq) — in-process `rep movsb` + VEH fault-safe host copies (`plaincopy.rs`, `0044-copy-plain.patch`).
- [edsuuu/VI-BR-PORT](https://github.com/edsuuu/VI-BR-PORT) — interval `CleanBlocks` (`ac6e475108`, based on `victory-fork#824`) and per-frame `Uncommitted` view-block memo (`136efde0ab`).
- Upstream PR contributors: `0xKaji` ([VIctory#2752](https://github.com/Marvisak/VIctory/pull/2752)), [VIctory#2735](https://github.com/Marvisak/VIctory/pull/2735), `spy0eye-beep` ([klbf-lab#23](https://github.com/PastiiJay/klbf-lab/pull/23)), & `Vladimir-Human` ([klbf-lab#16](https://github.com/PastiiJay/klbf-lab/pull/16)).

Licensed under **GPL-3.0-or-later**. See [LICENSE](LICENSE).

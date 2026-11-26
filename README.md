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

## Status (2026-11-26)

**Stage 6 — In-game (player-controlled gameplay verified)** on an 8 GB VRAM / 16 GB RAM PC.

Tested on: **Windows 11 x64 (`26200`)** · **AMD Radeon RX 6600 XT 8 GB** (RDNA2, Adrenalin **26.11.2** / `32.0.21045.29003`) · **AMD Ryzen 5 3600** (6C/12T) · **16 GB RAM** (pagefile on **M.2 NVMe SSD**) · **500 GB 5400 RPM SATA HDD** (game files on `D:`).

| Lane / Stage | Status | Details |
|---|---|---|
| **Boot & D3D12 init** | ✅ Works | Native Wave64 (`min=32 max=64 compute=64`); `0.5–1.2 ms` GPU; D3D12 Debug Layer verified (**533 / 533** tests green across `victory-shader`, `victory-gpu`, `victory-hle`, `victory-pipeline`) |
| **Setup, menus & pause UI (`series-s` + FSR)** | ✅ Works | **30.0 fps** in menus/loading, **22 fps** in pause menu (`VI` HUD, map/game/settings tabs, clean blur); scene preload gate with monotonic title clock freeze before prologue |
| **Offline pipeline pre-build** | ✅ Works | `27,144 / 27,144` pipelines built in `1,029 s` with **0 `amdxc64.dll` crashes** |
| **Player control (Stage 6)** | ✅ Verified | **Native DualShock 4 (`0x09CC`) HID input** with `VICTORY_ONE_PAD=1` reconnect-pause fix — walked & rotated camera (`L`/`R`) through prison yard/cell, interacted with cellmate (`LT`), followed guard, progressed through counselor's office, exterior/car scenes & cutscenes (with audio) |
| **Picture (in-world)** | 🟢 Natural lighting (no magenta) | `ambient.rs` (`level=0.6`, `reflections held=0.5`) eliminates AMD magenta RTGI & whitewashed reflections; 79 card-resident buffers (`657 MB`, `0.0 MB` spilled to CPU) restore full prison cell lighting & faces; Wave 4 adds 9-bit `exp prim` mesh index fix, waterfall SCC fix (`#3016`), and unaligned raw buffer view `own_bytes` fix (`226 -> 0` null views) |
| **Speed (in-world)** | 🟢 **5–10 fps** (`99–214 ms/frame`) | Measured on Wave 1+2 (`a4fb0c7`): **5 fps** (`214 ms`, `5,021 draws`) in heavy prison cell, **6–8 fps** (`130–160 ms`) outdoors & interiors, **10 fps** (`99–103 ms`) in car/cutscenes (`waiting for the card` dropped `126.6 ms -> 14.7 ms`, fetch waits `49.5 ms -> 0.0 ms`, `copying back` `132 ms -> 50 ms`). Wave 3 (`c0a3a1f`) further cuts CPU `heap_buffers`/`sync_heaps` + HDD streaming |
| **Stability** | ✅ **30–39 min** runs without a crash | Clean exits, `0 ERROR`, `0` unhandled exceptions, `0` device removals, `0` `amdxc64.dll` crashes; walking `E_INVALIDARG` crash fixed (`cf489e1`); pagefile moved to **M.2 NVMe SSD** |

Full session-by-session logs: [logs/results.md](logs/results.md).

---

## What `ruff-lab` adds on top of KLBF Lab 39

Most compatibility forks are tested on 32 GB RAM and 12–16 GB VRAM cards. `ruff-lab` focuses on making the runtime viable on **8 GB GPUs, 16 GB RAM hosts, and HDD storage**:

| Area | Problem on 8 GB / 16 GB / HDD | Fix in `ruff-lab` |
|---|---|---|
| **8 GB VRAM texture starvation** | `targets_room_kept()` hardcoded a `3,072 MiB` reserve (measured on 16 GB Series X runs), leaving only ~2.5 GB for textures on 8 GB cards (`no room for new ones so_far=25500`), and refusing textures at 90% VRAM left ground, hands, and character skin pitch black | Adaptive `default_targets_room_mb()` (14% / max `1,024 MiB` in Series S 720p), mip-dropped short-room fallback (`VICTORY_TEXTURE_SHORT=256` up to 97% VRAM, `ka1qux`), world-texture max-dimension cap with UI protection (`VICTORY_TEXTURE_MAX_DIM=1024` + `KEEP_UI`, `klbf-lab` Lab 62), LRU eviction (`VICTORY_TEXTURE_LRU=1`), and buffer-pass heap-drop cleanup (`VIniVIdiVIci`) |
| **Sky-dome AMD lighting & RT pass removal** | Without hardware RT, `CS_ScreenProbes_TemporalResolve` left bright magenta and pitch-black shadows; unused RT passes and ray-request feeders wasted GPU time and allocated RT-only textures | Upgraded `ambient.rs` from [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) (`4f62c2a`) so ambient light follows the sky dome dynamically (day/night, `VICTORY_AMBIENT_SKY=off` to keep constant) + clamps unoccluded reflections (`0.5`), and added `VICTORY_SKIP_RT=1` (default ON) skipping unused RT & ray-feeder compute passes (`Belobug` `6986a57` + `vilocity` `2681546`) so they cost 0 ms and allocate 0 RT textures |
| **16 GB RAM staging bloat** | Every sampled texture kept a full-size staging copy in system RAM, pushing commit to 12–20 GB and thrashing pagefiles | Integrated [PastiiJay/klbf-lab#23](https://github.com/PastiiJay/klbf-lab/pull/23) (by `spy0eye-beep`): releases staging buffers of unchanged textures after 2 s / 30 frames while keeping metadata |
| **6-core CPU descriptor lock contention & heap limits** | Global mutex on `HostHeap::mirror` stalled multi-threaded descriptor updates, descriptor tables were re-scanned every draw, and fixed 256K dynamic slots dropped draws on AMD's 2M view heap | Integrated [PastiiJay/klbf-lab#16](https://github.com/PastiiJay/klbf-lab/pull/16) (256 cache-aligned stripes + lock-free fast path), `VICTORY_DYNAMIC_SLOTS` (`klbf-lab` `4e95986`), pre-grouped `TablePlan` once per pipeline (`coolfox`/`VIctory-7900XTX` `46e3297`), one-time heap-cap warning ([VIctory#2634](https://github.com/Marvisak/VIctory/pull/2634)), and 64 KiB CBV cap with 0-byte CBV preservation |
| **Rotational HDD seek thrashing** | DirectStorage spawned `available_parallelism()` (12) concurrent file reader threads | Added `VICTORY_DSTORAGE_WORKERS` (`1..=64`, default = CPU count) in `crates/victory-hle/src/modules/dstorage.rs` so HDD users can set `2`–`4` workers |
| **Native Sony DS4 / DualSense input** | DualShock 4 & DualSense controllers required `x360ce` / `ViGEmBus` / `DS4Windows`, which often broke or failed to bind to `GameInput` | Native Windows HID reader (`crates/victory-hle/src/modules/gameinput/sony.rs`, adapted from [realblastex/flamingo](https://github.com/realblastex/flamingo)) supporting **DualShock 4** (v1/v2/dongle) & **DualSense / Edge** over USB and Bluetooth as slot 0 fallback (`VICTORY_SONY_PAD=0` to disable) |
| **BCPack fragment edge cases** | Corner-case single-fragment / zero-count BCPack textures could fail decompression | Backported [Marvisak/VIctory#2648](https://github.com/Marvisak/VIctory/pull/2648) & [#2649](https://github.com/Marvisak/VIctory/pull/2649) in `crates/victory-formats/src/bcpack.rs` |
| **Release binary CPU throughput** | Default release profile compiled for baseline SSE2 without LTO across crates | Enabled `lto = "thin"`, `codegen-units = 1` ([Belobug#96](https://github.com/Belobug/VIctory-Belobug-AMD/pull/96)), and `target-cpu=x86-64-v3` (AVX2/BMI2/FMA) on Windows ([VIctory#3011](https://github.com/Marvisak/VIctory/pull/3011)) |
| **Refused AMD shaders & RDNA 2 correctness** | 40+ shaders failed translation on uniform static-sampler branches or split wave64 lane masks; RDNA 2 `v_mad_legacy_f32` and `v_cmpx_*` VOP3/SDWA needed exact GFX10+ semantics | Ported static-sampler branch selection, integer-target blend bypass (`ddb73326b0`), split wave64 lane-mask recovery (`76749ce58d`) from [DexNom/VIctory](https://github.com/DexNom/VIctory), RDNA 2 fused `v_mad_legacy_f32` + FTZ (`victory-fork` `f6a09d3`), GFX10 `v_cmpx_*` EXEC decoding ([VIctory#3083](https://github.com/Marvisak/VIctory/pull/3083)), and 3D `DispatchMesh` (`VIctory#2624`) |
| **16 GB RAM commit pressure & pipeline workers** | Background pipeline workers spawned 6+ threads (~3–5 GB extra RAM), slow compute PSOs starved graphics draws, and normal exits could lose recent PSO cache writes | Added `VICTORY_PIPELINE_WORKERS` ([VIctory#2752](https://github.com/Marvisak/VIctory/pull/2752)), graphics-first worker priority reserving a worker from compute ([VIctory#3025](https://github.com/Marvisak/VIctory/pull/3025)), pipeline cache flush on normal exit ([VIctory#3046](https://github.com/Marvisak/VIctory/pull/3046)), and `VICTORY_COMMIT_RESERVE_MB` (`ka1qux`) |
| **CPU frame time: copies, hashing, binds & barriers** | `ReadProcessMemory` kernel crossings (`37–52 ms/frame`), `blake3` per-refresh texture hashing, repeated pipeline/RT/CBV/GPU-write declarations per list, per-buffer barriers, view-guard scans, and page remaps | Ported `plaincopy.rs` (`kelvaroq` `0044`), `xxh3-128` texture hashing ([VIctory#3011](https://github.com/Marvisak/VIctory/pull/3011)), `bound.rs` ([VIctory#3167](https://github.com/Marvisak/VIctory/pull/3167)), render-target bind cache (`VICTORY_BIND_CACHE`), root CBV scratch cache, `VICTORY_DECLARE_ONCE` (`cgbj`/`Belobug`/`ZJOU`), 8-way card compare (`9b90ad1`), `WaitOnAddress` fence wakes, XMA audio refill fix (`cgbj`), batched card-copy barriers (`773d902`), interval `CleanBlocks` + `Uncommitted` memo (`edsuuu`), `VICTORY_REMAP_LEAN=1` (`ka1qux`), and +27% BC7 BCPack lookup tables ([VIctory#2735](https://github.com/Marvisak/VIctory/pull/2735)) |
| **Waves 1–2: GPU settles, copy-back & masked uploads (`3f43861`, `a4fb0c7`, `bcca048`)** | `waiting for the card` (`126.6 ms/frame`), `copying back` (`132 ms`), relay fetch waits (`49.5 ms`), 351k tiny `CopyBufferRegion` calls/frame, 16-frame full 75k-slot `heap_buffers` rescan spikes, and `scene_burst` spam (`831/run`) | Added `CARD_PATCH` HLSL compute shader (`VICTORY_UNDER_WRITES_MASKS=1`, collapsing 351k tiny copies into 1–2 masked 4 KiB dispatches, D3D12 Debug Layer verified byte-exact on RX 6600 XT), `VICTORY_RANGED`, `VICTORY_COPY_CHANGED`, `VICTORY_CARD_FETCH_INFLIGHT=1` (`0.0 ms` fetch waits), `VICTORY_WIDE_THREADS`, rotating `1/64` incremental `heap_buffers` rescan (`VICTORY_HEAP_RESCAN_FRAMES=64`), shared stencil depth-twin scratch pool ([VIctory#2162](https://github.com/Marvisak/VIctory/pull/2162)), 500 ms `E_OUTOFMEMORY` retry backoff ([VIctory#3276](https://github.com/Marvisak/VIctory/pull/3276)), `SHADER_HEAPS` `RwLock` ([VIctory#2821](https://github.com/Marvisak/VIctory/pull/2821)), and parallel `sync_heaps()` |
| **Wave 3: Multi-threaded descriptor/heap CPU & HDD streaming (`c0a3a1f`)** | Single-threaded 35k-slot `heap_buffers` (`103–129 ms/frame`), double-enqueue of `self.changed` on `seen=true`, mutex contention on `placed`/`textures` in `sync_heaps()`, per-block `VirtualProtect`, per-draw `user_data` heap allocs, and serialized HDD reads (`FILE_FLAG_NO_BUFFERING` + sync DirectStorage handle) | Fixed `self.changed` re-enqueue on `seen=true`, added lock-free per-slot `slot_flags` (`SLOT_PLACED`/`SLOT_TEX`), parallelized `heap_buffers()` slot mirroring via `std::thread::scope`, added stack `[0u32; 64]` `user_data`, `HostList::states_generation` memoization ([VIctory#3249](https://github.com/Marvisak/VIctory/pull/3249) + `d4903fa`), lock-free `Shadow::pending_len`, pooled copy-back scratch & non-allocating `Shadow::writing` ([VIctory#2848](https://github.com/Marvisak/VIctory/pull/2848)/[#3318](https://github.com/Marvisak/VIctory/pull/3318)), coalesced `VirtualProtect` with 487 fallback, OS file cache for sync read-only packs (`VICTORY_PACK_CACHE`), and `FILE_FLAG_OVERLAPPED` concurrent DirectStorage reads ([VIctory#2427](https://github.com/Marvisak/VIctory/pull/2427)) |
| **Wave 4: Visual, shader & stability bugfixes (`cf489e1`, `b3073ea`, `3ea0d32`)** | `E_INVALIDARG` walking crash on mip-dropped textures, DS4 reconnect pause, 226 unaligned raw buffer views (`format=39 first=2 / 1615`) left `null`, GFX10 `exp prim` 10-bit mask culling valid mesh triangles, waterfall `s_andn2_wrexec` dropping 2 compute shaders, invalid IB leaks, worker panics, DSP race, and stale cache `.tmp` after kill | Fixed mip-dropped copy bounds (`cf489e1`), `VICTORY_ONE_PAD=1`, scene preload gate + monotonic clock freeze (`b3073ea`), `own_bytes()` 16-byte alignment copy for unaligned raw SRV/UAV (`226 -> 0` null views), 9-bit `exp prim` mask `0x1FF` (`c0228d9`), waterfall `s_andn2_wrexec` `SCC=false` ([VIctory#3016](https://github.com/Marvisak/VIctory/pull/3016)), IB validation ([VIctory#3183](https://github.com/Marvisak/VIctory/pull/3183)), pipeline worker `catch_unwind` guard (`#1353`), atomic `Status` + `Mutex` in `dsp.rs` ([VIctory#3319](https://github.com/Marvisak/VIctory/pull/3319)), and `.create(true).truncate(true)` cache staging (`#1321`) |

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

Run (`--console series-s` + AMD FSR, tuned for 8 GB VRAM / 16 GB RAM + M.2 pagefile / 6C12T CPU / HDD; add `$env:VICTORY_FRAMEGEN = '1'` for AMD FSR Frame Generation):

```powershell
$env:VICTORY_SHADER_OPT = '0'
$env:VICTORY_SKIP_PIPELINES = (Get-Content docs\klbf\skip-amd.txt -Raw).Trim()
$env:VICTORY_LATE_WAIT = '3'
$env:VICTORY_RELAY_COPY_QUEUE = '0'
$env:VICTORY_WIDE_BACK = '1'
$env:VICTORY_WIDE_MERGE = '1'
$env:VICTORY_WIDE_REFRESH = '1'
$env:VICTORY_WIDE_THREADS = '6'
$env:VICTORY_UNDER_WRITES_WORDS = '0'
$env:VICTORY_UNDER_WRITES_RUNS = '100000'
$env:VICTORY_UNDER_WRITES_MASKS = '1'
$env:VICTORY_UNDER_WRITES_MASKS_LEAST = '1'
$env:VICTORY_CARD_FETCH_INFLIGHT = '1'
$env:VICTORY_COPY_CHANGED = '1'
$env:VICTORY_RANGED = '1'
$env:VICTORY_HEAP_RESCAN_FRAMES = '64'
$env:VICTORY_CARD_BUFFERS_MB = '1024'
$env:VICTORY_CARD_REHASH_RUNS = '1'
$env:VICTORY_JOIN_COPIES = '1'
$env:VICTORY_DSTORAGE_WORKERS = '3'
$env:VICTORY_PIPELINE_WORKERS = '4'
$env:VICTORY_PIPELINE_WAIT_MS = '250'
$env:VICTORY_COMMIT_RESERVE_MB = '2048'
$env:VICTORY_REMAP_LEAN = '1'
$env:VICTORY_TEXTURE_MAX_DIM = '1024'
$env:VICTORY_TEXTURE_LRU = '1'
$env:VICTORY_TEXTURE_ROOM_PERCENT = '92'
$env:VICTORY_TEXTURE_KEEP_PERCENT = '78'
$env:VICTORY_DYNAMIC_SLOTS = '524288'
$env:VICTORY_DISPLAY_MODE = '1280x720'
$env:VICTORY_AMBIENT_SHADE = '0'
$env:VICTORY_FFX_DIR = '<FidelityFX SDK>\Kits\FidelityFX\signedbin'
.\target\release\victory.exe run --console series-s "<game_dir>\game_scarlett_master.exe" --execute
```

---

## Credits & License

- [Marvisak/VIctory](https://github.com/Marvisak/VIctory) — original Xbox Series X|S compatibility runtime for Windows.
- [fxceless/victory-fork](https://github.com/fxceless/victory-fork) — renderer and D3D12 work.
- [PastiiJay/klbf-lab](https://github.com/PastiiJay/klbf-lab) / [klbf-lab-beta](https://github.com/PastiiJay/klbf-lab-beta) — Lab 39 AMD fixes, Series S HUD, water physics, and FSR frame generation.
- [realblastex/flamingo](https://github.com/realblastex/flamingo) — native DualSense HID reader design adapted for DS4 + DualSense in `gameinput/sony.rs`.
- [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) — sky-dome-following ambient & reflection clamp pass (`ambient.rs`, `4f62c2a`), RT screen-probe activation skip (`6986a57`), and release LTO optimization (`#96`).
- [DexNom/VIctory](https://github.com/DexNom/VIctory) — uniform static-sampler branch lowering, integer-target blend bypass (`ddb73326b0`), and split wave64 lane-mask recovery (`76749ce58d`).
- [ka1qux/BRVIctory-ka1qux](https://github.com/ka1qux/BRVIctory-ka1qux) — short-room mip-dropped textures (`VICTORY_TEXTURE_SHORT` / `VICTORY_TEXTURE_DROP_MIPS`), Windows commit reserve (`VICTORY_COMMIT_RESERVE_MB`), lean page remaps (`VICTORY_REMAP_LEAN=1`, after `victory-fork#1069`), and 3D `DispatchMesh` forwarding (`VIctory#2624`).
- [fablinoo/kelvaroq](https://github.com/fablinoo/kelvaroq) — in-process `rep movsb` + VEH fault-safe host copies (`plaincopy.rs`, `0044-copy-plain.patch`).
- [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX) & `codec-d/cool_fox` — pre-grouped `TablePlan` (`46e3297`), batched/deduplicated card-copy barriers (`773d902`), and per-list pipeline bind deduplication (`bound.rs`).
- [voidopcode/vilocity](https://github.com/voidopcode/vilocity) — unused ray-tracing & ray-feeder pass skip patterns (`2681546144`).
- [KomanderDG88/VIniVIdiVIci](https://github.com/KomanderDG88/VIniVIdiVIci) — buffer-pass copy cleanup on descriptor heap drop (`82d246c`).
- [edsuuu/VI-BR-PORT](https://github.com/edsuuu/VI-BR-PORT) — interval `CleanBlocks` (`ac6e475108`, based on `victory-fork#824`) and per-frame `Uncommitted` view-block memo (`136efde0ab`).
- Upstream PR contributors: `0xKaji` ([VIctory#2752](https://github.com/Marvisak/VIctory/pull/2752)), [VIctory#2735](https://github.com/Marvisak/VIctory/pull/2735), `0xkn0wh3r3` ([VIctory#3011](https://github.com/Marvisak/VIctory/pull/3011)), `Luislariosss` ([VIctory#3025](https://github.com/Marvisak/VIctory/pull/3025)), `fuzeofficial` ([VIctory#3046](https://github.com/Marvisak/VIctory/pull/3046)), `pes7` ([VIctory#2634](https://github.com/Marvisak/VIctory/pull/2634)), `RVZO6` ([VIctory#3083](https://github.com/Marvisak/VIctory/pull/3083)), `pablogonz12` ([VIctory#3167](https://github.com/Marvisak/VIctory/pull/3167)), `spy0eye-beep` ([klbf-lab#23](https://github.com/PastiiJay/klbf-lab/pull/23)), & `Vladimir-Human` ([klbf-lab#16](https://github.com/PastiiJay/klbf-lab/pull/16)).

Licensed under **GPL-3.0-or-later**. See [LICENSE](LICENSE).

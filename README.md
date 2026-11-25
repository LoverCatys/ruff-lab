<h1 align="center">ruff-lab</h1>

<p align="center">
  <b>Testing & benchmarking Grand Theft Auto VI (Xbox Series X|S build) on mid-range AMD RDNA2 + 16 GB RAM PC hardware.</b>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/GPU-AMD%20RX%206600%20XT%208GB-ed1c24" alt="GPU: AMD RX 6600 XT 8GB">
  <img src="https://img.shields.io/badge/CPU-Ryzen%205%203600-ff6600" alt="CPU: Ryzen 5 3600">
  <img src="https://img.shields.io/badge/RAM-16%20GB-1f46c8" alt="RAM: 16 GB">
  <img src="https://img.shields.io/badge/Driver-Adrenalin%2026.11.2-333" alt="Driver: Adrenalin 26.11.2">
</p>

> [!NOTE]
> **No game files, dumps, or binaries are included in this repository.** This repo only tracks test runs, configurations, frame rates, stability notes, and bug reports.

---

## Test Hardware & Environment

| Component | Spec |
|---|---|
| **OS** | Windows 11 x64 (`10.0.26200`) |
| **CPU** | AMD Ryzen 5 3600 (6C / 12T, AVX2) |
| **GPU** | AMD Radeon RX 6600 XT 8 GB (RDNA2, native Wave64) |
| **GPU Driver** | AMD Software: Adrenalin Edition **26.11.2** (`32.0.21045.29003`) |
| **RAM** | 16 GB DDR4 (32 GB total virtual / pagefile) |
| **Storage (Game)** | 500 GB SATA HDD (`WDC WD5000LPCX-00VHAT0`, 5400 RPM) |
| **Runtime Layer** | [PastiiJay/klbf-lab-beta](https://github.com/PastiiJay/klbf-lab-beta) (`klbf-amd` / **Lab 39**, commit `0ad4037`, fork of [Marvisak/VIctory](https://github.com/Marvisak/VIctory)) |
| **Upscaler** | AMD FidelityFX SDK v2.4.0 (`amd_fidelityfx_loader_dx12.dll`) |

---

## Current Status (2026-11-25)

| Stage | Status | Notes |
|---|---|---|
| Build & PE/HLE verification | ✅ Passed | `victory.exe` built cleanly with Rust `1.98.1` (`x86_64-pc-windows-msvc`); all 587 imports resolved |
| Boot & D3D12 init | ✅ Works | Native Wave64 detected (`min=32 max=64 compute=64`); ~0.5–1.2 ms GPU time |
| First-boot setup, menus & HUD (`series-s` + FSR) | ✅ Works | **30.0 fps**; setup screens, character loading art & spinner render cleanly in Lab 39 |
| World loading (from 5400 RPM HDD) | ✅ Reaches world | Reaches `Game Running` / Mission `0x98c85f2a` (`TileHeap 2090/3977 MB`, RAM `5.88 GB` / commit `8.66 GB`); ~12–14 min on HDD |
| Prologue (in-world) | ⏳ Next | Hit 30 s RAGE watchdog (`+0x23cf683 → +0x2215c35`) on first world frame while compiling pipelines on the fly (`VICTORY_PIPELINE_GATE_S`); zero `amdxc64.dll` crashes. Offline pipeline pre-build in progress (~85%, 0 crashes) |
| Mission 1 ("Gone Fishing") | ⏳ Pending | — |

---

## Launch Configuration

Recommended AMD RDNA2 + 16 GB RAM baseline (`--console series-s`, 720p → 1080p via AMD FSR):

```powershell
$env:VICTORY_SHADER_OPT = '0'
$env:VICTORY_SKIP_PIPELINES = (Get-Content docs\klbf\skip-amd.txt -Raw).Trim()
$env:VICTORY_LATE_WAIT = '1'
$env:VICTORY_WIDE_BACK = '1'
$env:VICTORY_PIPELINE_WAIT_MS = '24'
$env:VICTORY_RELAY_COPY_QUEUE = '0'
$env:VICTORY_FFX_DIR = 'D:\GTA6\FidelityFX-SDK\Kits\FidelityFX\signedbin'

.\target\release\victory.exe run --console series-s "<game_dir>\game_scarlett_master.exe" --execute
```

---

## Run Log

Full session-by-session results are tracked in [logs/results.md](logs/results.md).

## Credits

- [Marvisak/VIctory](https://github.com/Marvisak/VIctory) — original Xbox Series X|S compatibility runtime for Windows.
- [PastiiJay/klbf-lab](https://github.com/PastiiJay/klbf-lab) — AMD-focused fork and tested builds.
- [AMD FidelityFX SDK](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK) — official DX12 FSR upscaler DLLs.

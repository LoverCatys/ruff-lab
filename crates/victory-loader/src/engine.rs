//! Byte-verified runtime engine overrides for `game_scarlett_master.exe` (1.0.2.1)
//! inside `CSettingsManager::ApplySettings` (`+0x22434a0`), adapted from
//! `ToLIManl/VIctory-Hydra` Stage 14 (`ecbd78be`, GPL-3.0-or-later).
//!
//! Each patch site checks the exact original x86-64 bytes before writing; if a
//! different title or build is loaded, no bytes are modified.
//!
//! Environment variables (all off by default unless set):
//! - `VICTORY_LOD_SCALE`: `0`/`off`/`stock` | `low` (0.25) | `med` (0.35) | `high` (0.50) | `0.05..=1.0`
//! - `VICTORY_SMALL_CULL`: `0`/`off`/`stock` | `med` (2.0) | `high`/`perf` (3.0) | `max`/`ultra` (4.5) | `0.25..=16.0`
//! - `VICTORY_VEHICLE_LOD_BIAS`: `0`/`off`/`stock` | `-2.0..=4.0`

/// A byte-verified x86-64 patch site inside the mapped main executable.
#[derive(Clone, Copy, Debug)]
pub struct Site {
    pub at: usize,
    pub what: &'static str,
    pub bytes: &'static [u8],
}

pub const LOD_SCALE_GLOBAL_RVA: usize = 0xa15e200;
pub const SMALL_CULL_GLOBAL_RVA: usize = 0x684984c;
pub const VEHICLE_LOD_BIAS_GLOBAL_RVA: usize = 0x8fe0850;
pub const VEHICLE_LOD_BIAS_SETTER_RVA: usize = 0x5eef340;

pub const LOD_SCALE: Site = Site {
    at: 0x224360a,
    what: "world LOD scale override",
    bytes: &[
        0xc5, 0xfa, 0x10, 0x86, 0xd4, 0x00, 0x00, 0x00, // vmovss xmm0, [rsi+0xd4]
        0xc5, 0xfa, 0x11, 0x05, 0xe6, 0xab, 0xf1, 0x07, // vmovss [+0xa15e200], xmm0
    ],
};

pub const SMALL_CULL: Site = Site {
    at: 0x2243651,
    what: "small-object cull threshold override",
    bytes: &[
        0xc5, 0xfa, 0x10, 0x86, 0x0c, 0x01, 0x00, 0x00, // vmovss xmm0, [rsi+0x10c]
        0xc5, 0xfa, 0x11, 0x05, 0xeb, 0x61, 0x60, 0x04, // vmovss [+0x684984c], xmm0
    ],
};

pub const VEHICLE_LOD_BIAS: Site = Site {
    at: 0x2243720,
    what: "vehicle LOD bias override",
    bytes: &[
        0xc5, 0xfa, 0x10, 0x86, 0x84, 0x01, 0x00, 0x00, // vmovss xmm0, [rsi+0x184]
        0xe8, 0x13, 0xbc, 0xca, 0x03, // call +0x5eef340
    ],
};

pub fn parse_lod_scale(spec: &str) -> Option<f32> {
    let s = spec.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("stock") || s.eq_ignore_ascii_case("off") || s == "0" {
        return None;
    }
    match s.to_ascii_lowercase().as_str() {
        "low" => Some(0.25),
        "med" | "medium" => Some(0.35),
        "high" => Some(0.50),
        _ => s.parse::<f32>().ok().filter(|v| v.is_finite() && (0.05..=1.0).contains(v)),
    }
}

pub fn parse_small_cull(spec: &str) -> Option<f32> {
    let s = spec.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("stock") || s.eq_ignore_ascii_case("off") || s == "0" {
        return None;
    }
    match s.to_ascii_lowercase().as_str() {
        "med" | "medium" => Some(2.0),
        "high" | "perf" => Some(3.0),
        "max" | "ultra" => Some(4.5),
        _ => s.parse::<f32>().ok().filter(|v| v.is_finite() && (0.25..=16.0).contains(v)),
    }
}

pub fn parse_vehicle_lod_bias(spec: &str) -> Option<f32> {
    let s = spec.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("stock") || s.eq_ignore_ascii_case("off") || s == "0" {
        return None;
    }
    s.parse::<f32>().ok().filter(|v| v.is_finite() && (-2.0..=4.0).contains(v))
}

pub fn lod_scale_code(scale: f32) -> Vec<u8> {
    let disp = (LOD_SCALE_GLOBAL_RVA as i64 - (LOD_SCALE.at + 5 + 6) as i64) as i32;
    let mut code = Vec::with_capacity(16);
    code.push(0xb8); // mov eax, imm32
    code.extend_from_slice(&scale.to_bits().to_le_bytes());
    code.extend_from_slice(&[0x89, 0x05]); // mov dword ptr [rip+disp], eax
    code.extend_from_slice(&disp.to_le_bytes());
    code.extend_from_slice(&[0xc5, 0xf9, 0x6e, 0xc0]); // vmovd xmm0, eax
    code.push(0x90); // nop
    code
}

pub fn small_cull_code(threshold: f32) -> Vec<u8> {
    let disp = (SMALL_CULL_GLOBAL_RVA as i64 - (SMALL_CULL.at + 10) as i64) as i32;
    let mut code = Vec::with_capacity(16);
    code.extend_from_slice(&[0xc7, 0x05]); // mov dword ptr [rip+disp], imm32
    code.extend_from_slice(&disp.to_le_bytes());
    code.extend_from_slice(&threshold.to_bits().to_le_bytes());
    code.extend_from_slice(&[0x90; 6]);
    code
}

pub fn vehicle_lod_bias_code(bias: f32) -> Vec<u8> {
    let disp = (VEHICLE_LOD_BIAS_GLOBAL_RVA as i64 - (VEHICLE_LOD_BIAS.at + 10) as i64) as i32;
    let mut code = Vec::with_capacity(13);
    code.extend_from_slice(&[0xc7, 0x05]); // mov dword ptr [rip+disp], imm32
    code.extend_from_slice(&disp.to_le_bytes());
    code.extend_from_slice(&bias.to_bits().to_le_bytes());
    code.extend_from_slice(&[0x90; 3]);
    code
}

/// Applies a single byte-verified patch into `buffer` (used in tests and on the mapped image).
#[cfg(test)]
fn verify_and_copy(dst: &mut [u8], site: &Site, code: &[u8]) -> Result<(), &'static str> {
    if code.len() != site.bytes.len() {
        return Err("replacement length mismatch");
    }
    let Some(end) = site.at.checked_add(site.bytes.len()) else {
        return Err("site offset overflow");
    };
    if end > dst.len() {
        return Err("site outside image");
    }
    if &dst[site.at..end] != site.bytes {
        return Err("original bytes did not match expected build");
    }
    dst[site.at..end].copy_from_slice(code);
    Ok(())
}

#[cfg(windows)]
fn apply_site(base: usize, image_size: usize, site: &Site, code: &[u8]) -> bool {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    if code.len() != site.bytes.len() {
        return false;
    }
    let Some(end) = site.at.checked_add(site.bytes.len()) else {
        return false;
    };
    if end > image_size {
        tracing::warn!(what = site.what, at = format_args!("{:#x}", site.at), image_size, "engine patch skipped: outside image");
        return false;
    }
    let addr = base + site.at;
    // SAFETY: `[base, base + image_size)` is the freshly mapped title image and no title code is running yet.
    let current = unsafe { std::slice::from_raw_parts(addr as *const u8, site.bytes.len()) };
    if current != site.bytes {
        tracing::warn!(what = site.what, at = format_args!("{:#x}", site.at), "engine patch skipped: image bytes did not match");
        return false;
    }
    let mut old = 0u32;
    // SAFETY: temporarily makes the verified code slice writable, copies replacement bytes, restores protection, and flushes I-cache.
    unsafe {
        if VirtualProtect(addr as *const _, code.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            tracing::warn!(what = site.what, error = %std::io::Error::last_os_error(), "engine patch VirtualProtect failed");
            return false;
        }
        std::ptr::copy_nonoverlapping(code.as_ptr(), addr as *mut u8, code.len());
        VirtualProtect(addr as *const _, code.len(), old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), addr as *const _, code.len());
    }
    true
}

/// Applies any requested RAGE 9 LOD/cull overrides (`VICTORY_LOD_SCALE`,
/// `VICTORY_SMALL_CULL`, `VICTORY_VEHICLE_LOD_BIAS`) to the mapped main executable.
#[cfg(windows)]
pub fn apply_asked(base: usize, image_size: usize) {
    if let Ok(spec) = std::env::var("VICTORY_LOD_SCALE")
        && let Some(scale) = parse_lod_scale(&spec)
        && apply_site(base, image_size, &LOD_SCALE, &lod_scale_code(scale))
    {
        tracing::info!(scale, rva = format_args!("{:#x}", LOD_SCALE.at), "engine patch applied: VICTORY_LOD_SCALE");
    }
    if let Ok(spec) = std::env::var("VICTORY_SMALL_CULL")
        && let Some(threshold) = parse_small_cull(&spec)
        && apply_site(base, image_size, &SMALL_CULL, &small_cull_code(threshold))
    {
        tracing::info!(threshold, rva = format_args!("{:#x}", SMALL_CULL.at), "engine patch applied: VICTORY_SMALL_CULL");
    }
    if let Ok(spec) = std::env::var("VICTORY_VEHICLE_LOD_BIAS")
        && let Some(bias) = parse_vehicle_lod_bias(&spec)
        && apply_site(base, image_size, &VEHICLE_LOD_BIAS, &vehicle_lod_bias_code(bias))
    {
        tracing::info!(bias, rva = format_args!("{:#x}", VEHICLE_LOD_BIAS.at), "engine patch applied: VICTORY_VEHICLE_LOD_BIAS");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsers_validate_presets_and_bounds() {
        assert_eq!(parse_lod_scale("0"), None);
        assert_eq!(parse_lod_scale("stock"), None);
        assert_eq!(parse_lod_scale("low"), Some(0.25));
        assert_eq!(parse_lod_scale("med"), Some(0.35));
        assert_eq!(parse_lod_scale("0.42"), Some(0.42));
        assert_eq!(parse_lod_scale("0.01"), None);
        assert_eq!(parse_lod_scale("1.5"), None);

        assert_eq!(parse_small_cull("off"), None);
        assert_eq!(parse_small_cull("med"), Some(2.0));
        assert_eq!(parse_small_cull("perf"), Some(3.0));
        assert_eq!(parse_small_cull("2.5"), Some(2.5));
        assert_eq!(parse_small_cull("20.0"), None);

        assert_eq!(parse_vehicle_lod_bias("0"), None);
        assert_eq!(parse_vehicle_lod_bias("0.5"), Some(0.5));
        assert_eq!(parse_vehicle_lod_bias("-1.0"), Some(-1.0));
        assert_eq!(parse_vehicle_lod_bias("5.0"), None);
    }

    #[test]
    fn generated_patches_match_lengths_and_rip_targets() {
        let c1 = lod_scale_code(0.35);
        assert_eq!(c1.len(), LOD_SCALE.bytes.len());
        let d1 = i32::from_le_bytes(c1[7..11].try_into().unwrap());
        assert_eq!((LOD_SCALE.at + 11) as i64 + i64::from(d1), LOD_SCALE_GLOBAL_RVA as i64);
        assert_eq!(&c1[1..5], &0.35f32.to_bits().to_le_bytes());

        let c2 = small_cull_code(2.5);
        assert_eq!(c2.len(), SMALL_CULL.bytes.len());
        let d2 = i32::from_le_bytes(c2[2..6].try_into().unwrap());
        assert_eq!((SMALL_CULL.at + 10) as i64 + i64::from(d2), SMALL_CULL_GLOBAL_RVA as i64);
        assert_eq!(&c2[6..10], &2.5f32.to_bits().to_le_bytes());

        let c3 = vehicle_lod_bias_code(0.5);
        assert_eq!(c3.len(), VEHICLE_LOD_BIAS.bytes.len());
        let d3 = i32::from_le_bytes(c3[2..6].try_into().unwrap());
        assert_eq!((VEHICLE_LOD_BIAS.at + 10) as i64 + i64::from(d3), VEHICLE_LOD_BIAS_GLOBAL_RVA as i64);
        assert_eq!(&c3[6..10], &0.5f32.to_bits().to_le_bytes());
    }

    #[test]
    fn verify_and_copy_refuses_mismatched_bytes() {
        let site = Site { at: 4, what: "test", bytes: &[1, 2, 3, 4] };
        let mut img = vec![0u8; 16];
        assert!(verify_and_copy(&mut img, &site, &[9, 8, 7, 6]).is_err());
        img[4..8].copy_from_slice(&[1, 2, 3, 4]);
        assert!(verify_and_copy(&mut img, &site, &[9, 8, 7, 6]).is_ok());
        assert_eq!(&img[4..8], &[9, 8, 7, 6]);
    }
}

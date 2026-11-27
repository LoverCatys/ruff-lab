//! Samplers as the host describes them, from the console's sampler
//! description (`CreateSamplerX`), which names the hardware's sampler
//! fields. (Immediate samplers in shader code come decoded with the
//! translation.)

use windows::Win32::Graphics::Direct3D12::*;

/// The fields both forms share.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fields {
    /// Point 0, bilinear 1, anisotropic point 2, anisotropic bilinear 3.
    mag: u32,
    min: u32,
    /// None 0, point 1, linear 2.
    mip: u32,
    /// Plain 0, minimum 1, maximum 2.
    mode: u32,
    /// Host address modes.
    address: [u32; 3],
    anisotropy: u32,
    /// Host comparison function.
    compare: u32,
    min_lod: f32,
    max_lod: f32,
    bias: f32,
    border: [f32; 4],
}

impl Fields {
    fn host(&self) -> D3D12_SAMPLER_DESC {
        let linear = |f: u32| u32::from(f == 1 || f == 3);
        let aniso = self.mag >= 2 || self.min >= 2;
        // The console compares when the instruction says so, with the
        // sampler's function; the host needs a comparison sampler for that.
        // A sampler given a real test (less ... greater-or-equal) is one;
        // never, always and none are plain samplers.
        const LESS: u32 = 2;
        const GREATER_EQUAL: u32 = 7;
        let compares = (LESS..=GREATER_EQUAL).contains(&self.compare);
        let reduction = match self.mode {
            1 => 2,
            2 => 3,
            _ => u32::from(compares),
        };
        let filter = if aniso {
            0x55
        } else {
            linear(self.min) << 4 | linear(self.mag) << 2 | u32::from(self.mip == 2)
        };
        D3D12_SAMPLER_DESC {
            Filter: D3D12_FILTER((reduction << 7 | filter) as i32),
            AddressU: D3D12_TEXTURE_ADDRESS_MODE(self.address[0] as i32),
            AddressV: D3D12_TEXTURE_ADDRESS_MODE(self.address[1] as i32),
            AddressW: D3D12_TEXTURE_ADDRESS_MODE(self.address[2] as i32),
            MipLODBias: self.bias,
            MaxAnisotropy: if aniso {
                self.anisotropy.clamp(1, 16)
            } else {
                1
            },
            ComparisonFunc: D3D12_COMPARISON_FUNC(self.compare.max(1) as i32),
            BorderColor: self.border,
            MinLOD: self.min_lod,
            // Without a mip filter the console samples mip 0 only.
            MaxLOD: if self.mip == 0 {
                self.min_lod
            } else {
                self.max_lod
            },
        }
    }
}

/// The console's sampler description (27 words): flags, magnification,
/// minification, mip and depth filters, filter mode, three address modes,
/// minimum and maximum LOD, LOD bias, secondary bias, anisotropy, its bias
/// and threshold, comparison, border color, then tuning words.
pub(crate) fn console(words: &[u32; 27]) -> D3D12_SAMPLER_DESC {
    let f = |i: usize| f32::from_bits(words[i]);
    Fields {
        mag: words[1],
        min: words[2],
        mip: words[3],
        mode: words[5],
        address: [words[6], words[7], words[8]],
        min_lod: f(9),
        max_lod: f(10),
        bias: f(11),
        anisotropy: words[13],
        compare: words[16],
        border: [f(17), f(18), f(19), f(20)],
    }
    .host()
}

/// Applies low-graphics sampler downgrades (`VICTORY_LOW_GRAPHICS=1` or `2`):
/// disables anisotropic filtering and adds a positive mip LOD bias on plain
/// (non-comparison, non-min/max) multi-mip samplers for softer/blurrier
/// textures and lower bandwidth without affecting depth/shadow/UI/LUT samplers.
pub(crate) fn apply_low_graphics(desc: &mut D3D12_SAMPLER_DESC) {
    apply_low_graphics_level(desc, crate::host::low_graphics());
}

fn apply_low_graphics_level(desc: &mut D3D12_SAMPLER_DESC, level: u8) {
    if level == 0 {
        return;
    }
    // Only touch plain samplers (reduction == 0); keep comparison & min/max intact.
    if (desc.Filter.0 >> 7) != 0 {
        return;
    }
    let mode = desc.Filter.0 & 0x7f;
    if mode == 0x55 {
        // ANISOTROPIC -> trilinear (level 1) or bilinear (level 2).
        desc.Filter = D3D12_FILTER(if level >= 2 { 0x14 } else { 0x15 });
        desc.MaxAnisotropy = 1;
    } else if level >= 2 && mode == 0x15 {
        // Trilinear -> bilinear in ultra-low potato mode.
        desc.Filter = D3D12_FILTER(0x14);
    }
    if mode != 0 && desc.MaxLOD > desc.MinLOD {
        let extra = if level >= 2 { 3.0 } else { 1.5 };
        desc.MipLODBias = (desc.MipLODBias + extra).clamp(-16.0, 15.99);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_descriptions_decode() {
        let mut w = [0u32; 27];
        w[1] = 1; // bilinear magnification
        w[2] = 0; // point minification
        w[3] = 2; // linear mips
        w[6..9].copy_from_slice(&[3, 3, 3]);
        w[10] = 1000.0f32.to_bits();
        let d = console(&w);
        assert_eq!(d.Filter.0, 0x5, "min point, mag and mip linear");
        assert_eq!(d.AddressW, D3D12_TEXTURE_ADDRESS_MODE_CLAMP);
        assert_eq!(d.MaxLOD, 1000.0);
        // A less-or-equal test makes a comparison sampler; never does not.
        w[16] = 4;
        assert_eq!(console(&w).Filter.0, 0x85);
        assert_eq!(console(&w).ComparisonFunc, D3D12_COMPARISON_FUNC_LESS_EQUAL);
        w[16] = 1;
        assert_eq!(console(&w).Filter.0, 0x5);
    }

    #[test]
    fn low_graphics_downgrades_aniso_and_biases_plain_samplers_only() {
        let mut aniso = D3D12_SAMPLER_DESC {
            Filter: D3D12_FILTER(0x55),
            MaxAnisotropy: 16,
            MipLODBias: 0.0,
            MinLOD: 0.0,
            MaxLOD: 1000.0,
            ..Default::default()
        };
        apply_low_graphics_level(&mut aniso, 1);
        assert_eq!(aniso.Filter.0, 0x15);
        assert_eq!(aniso.MaxAnisotropy, 1);
        assert!((aniso.MipLODBias - 1.5).abs() < 1e-6);

        let mut potato = D3D12_SAMPLER_DESC {
            Filter: D3D12_FILTER(0x55),
            MaxAnisotropy: 16,
            MipLODBias: 14.0,
            MinLOD: 0.0,
            MaxLOD: 1000.0,
            ..Default::default()
        };
        apply_low_graphics_level(&mut potato, 2);
        assert_eq!(potato.Filter.0, 0x14);
        assert_eq!(potato.MaxAnisotropy, 1);
        assert!((potato.MipLODBias - 15.99).abs() < 1e-5);

        // Comparison sampler must remain untouched.
        let mut cmp = D3D12_SAMPLER_DESC {
            Filter: D3D12_FILTER(0x95),
            MaxAnisotropy: 1,
            MipLODBias: 0.0,
            MinLOD: 0.0,
            MaxLOD: 1000.0,
            ..Default::default()
        };
        apply_low_graphics_level(&mut cmp, 2);
        assert_eq!(cmp.Filter.0, 0x95);
        assert_eq!(cmp.MipLODBias, 0.0);

        // Single-mip point sampler (UI/LUT) must remain untouched.
        let mut ui = D3D12_SAMPLER_DESC {
            Filter: D3D12_FILTER(0x0),
            MaxAnisotropy: 1,
            MipLODBias: 0.0,
            MinLOD: 0.0,
            MaxLOD: 0.0,
            ..Default::default()
        };
        apply_low_graphics_level(&mut ui, 2);
        assert_eq!(ui.Filter.0, 0x0);
        assert_eq!(ui.MipLODBias, 0.0);
    }
}

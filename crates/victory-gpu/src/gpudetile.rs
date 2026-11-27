//! Texture detiling on the card (`VICTORY_GPU_DETILE`).
//! Adapted from `fxceless/victory-fork` (`3461f52`, GPL-3.0-or-later), extended
//! to support mip-dropped textures (`dropped > 0`) and bounded VRAM pools on
//! 8 GB GPUs.
//!
//! Without it a title texture's memory is detiled on the CPU element run by
//! element run. With it the upload staging buffer holds the texture's tiled
//! bytes (one linear `memcpy` on the CPU), and a compute pass in the command
//! list's prelude detiles them on the GPU into a device-local linear buffer
//! matching the host texture's copyable footprints.

use std::sync::OnceLock;

use victory_formats::texture::Layout;
use windows::Win32::Graphics::Direct3D12::*;

/// `VICTORY_GPU_DETILE`: unset or `1` = ON (default in Wave 6.5); `0` = OFF
/// (CPU detiling); `alt` = 20 s A/B toggle for measurements.
pub(crate) fn on() -> bool {
    static MODE: OnceLock<u8> = OnceLock::new();
    match *MODE.get_or_init(|| match std::env::var("VICTORY_GPU_DETILE").as_deref().map(str::trim) {
        Ok("0") => 0,
        Ok("alt") => 2,
        _ => 1,
    }) {
        0 => false,
        1 => true,
        _ => {
            static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
            let on = (EPOCH.get_or_init(std::time::Instant::now).elapsed().as_secs() / 20) % 2 == 1;
            static LAST: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);
            if LAST.swap(u8::from(on), std::sync::atomic::Ordering::Relaxed) != u8::from(on) {
                tracing::info!(
                    "title textures made from now on are detiled {} (VICTORY_GPU_DETILE=alt)",
                    if on { "ON THE CARD" } else { "on the PC, as usual" }
                );
            }
            on
        }
    }
}

/// The root constants of one dispatch (see [`SOURCE`]'s cbuffer).
pub(crate) const CONSTANTS: usize = 22;

/// Compute shader detiling one subresource per dispatch. One thread writes one
/// 32-bit word of a row of the linear subresource. Root SRV: upload buffer
/// holding tiled bytes + held-level mask + pattern tables; root UAV: linear
/// device-local buffer.
pub(crate) const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=22, b0), SRV(t0), UAV(u0)"
cbuffer P : register(b0) {
    uint flags;       // bit 0: linear
    uint n;           // element bytes: 1, 2, 4, 8, 16
    uint width;       // elements
    uint height;
    uint depth;
    uint first;       // the array slice
    uint slab;        // depth slices a slice of the layout holds
    uint slice_size;
    uint mip_offset;
    uint pitch;       // the mip's padded width in elements
    uint bw;
    uint bh;
    uint log2;        // log2 of the block's bytes
    uint tx;          // the mip's place in the tail block
    uint ty;
    uint tables;      // byte offset of the x table (y, then z, follow)
    uint dst;         // byte offset of the subresource in the linear buffer
    uint row_pitch;
    uint depth_pitch;
    uint words;       // 32-bit words a row of the subresource holds
    uint mask_at;     // byte offset of the held levels' mask
    uint mip;
};
ByteAddressBuffer src : register(t0);
RWByteAddressBuffer out_buf : register(u0);

uint element(uint x, uint y, uint z) {
    uint s = (first + z) / slab;
    uint inner = (first + z) % slab;
    uint base = s * slice_size + mip_offset;
    if ((flags & 1u) != 0u) {
        return base + (y * pitch + x) * n;
    }
    x += tx;
    y += ty;
    uint row = ((y / bh) * (pitch / bw)) << log2;
    uint col = (x / bw) << log2;
    uint px = src.Load(tables + 4u * (x % bw));
    uint py = src.Load(tables + 4u * (bw + (y % bh)));
    uint pz = src.Load(tables + 4u * (bw + bh + inner));
    return base + row + col + (px ^ py ^ pz);
}

[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= words || id.y >= height || id.z >= depth) return;
    uint at = dst + id.z * depth_pitch + id.y * row_pitch + id.x * 4u;
    uint held = src.Load(mask_at);
    if (mip < 32u && ((held >> mip) & 1u) == 0u) {
        out_buf.Store(at, 0u);
        return;
    }
    uint v = 0u;
    if (n >= 4u) {
        uint per = n >> 2;
        v = src.Load(element(id.x / per, id.y, id.z) + 4u * (id.x % per));
    } else {
        uint per = 4u / n;
        uint keep = n == 1u ? 0xffu : 0xffffu;
        [unroll]
        for (uint k = 0u; k < 4u; k++) {
            uint x = id.x * per + k;
            if (k < per && x < width) {
                uint a = element(x, id.y, id.z);
                uint w = src.Load(a & ~3u);
                v |= ((w >> ((a & 3u) * 8u)) & keep) << (k * n * 8u);
            }
        }
    }
    out_buf.Store(at, v);
}
"#;

/// The shader's root signature and pipeline.
pub(crate) struct Detiler {
    pub(crate) root: ID3D12RootSignature,
    pub(crate) pso: ID3D12PipelineState,
}

// SAFETY: immutable D3D12 objects, which are free-threaded.
unsafe impl Send for Detiler {}
// SAFETY: as above.
unsafe impl Sync for Detiler {}

/// Compiles [`SOURCE`] (`cs_5_1`, root signature embedded) on `device`.
pub(crate) fn make(device: &ID3D12Device) -> windows::core::Result<Detiler> {
    use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: source and names live through the call; out pointers are ours.
    let compiled = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            windows::core::s!("texture detile"),
            None,
            None,
            windows::core::s!("main"),
            windows::core::s!("cs_5_1"),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(e) = compiled {
        if let Some(errors) = errors {
            // SAFETY: the blob's own bytes.
            let text = unsafe {
                std::slice::from_raw_parts(
                    errors.GetBufferPointer().cast::<u8>(),
                    errors.GetBufferSize(),
                )
            };
            tracing::warn!("the texture detile shader: {}", String::from_utf8_lossy(text));
        }
        return Err(e);
    }
    let code = code.ok_or_else(windows::core::Error::empty)?;
    // SAFETY: the blob's own bytes; root signature embedded in the shader.
    unsafe {
        let bytes = std::slice::from_raw_parts(
            code.GetBufferPointer().cast::<u8>(),
            code.GetBufferSize(),
        );
        let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes)?;
        let pso: ID3D12PipelineState =
            device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                CS: D3D12_SHADER_BYTECODE {
                    pShaderBytecode: bytes.as_ptr().cast(),
                    BytecodeLength: bytes.len(),
                },
                ..Default::default()
            })?;
        Ok(Detiler { root, pso })
    }
}

/// The host's detiler, built on first use; `None` falls back to CPU detiling.
pub(crate) fn detiler() -> Option<&'static Detiler> {
    static MADE: OnceLock<Option<Detiler>> = OnceLock::new();
    MADE.get_or_init(|| {
        let host = crate::host::host()?;
        match make(&host.device) {
            Ok(d) => {
                tracing::info!("title textures are detiled on the card (VICTORY_GPU_DETILE)");
                Some(d)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "VICTORY_GPU_DETILE: the detile shader could not be made; textures are detiled on the PC"
                );
                None
            }
        }
    })
    .as_ref()
}

/// Layout inside a GPU-detiled upload staging buffer: `[0..tiled_bytes]` holds
/// the tiled title bytes, followed by the 4-byte held-level mask and pattern tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Raw {
    pub(crate) tiled_bytes: usize,
    pub(crate) mask_at: u64,
    pub(crate) tables_at: u64,
    pub(crate) bytes: u64,
}

pub(crate) fn raw_layout(layout: &Layout, read_size: usize) -> Raw {
    let tiled_bytes = read_size.min(layout.size as usize);
    let mask_at = (tiled_bytes as u64).next_multiple_of(16);
    let tables_at = mask_at + 16;
    let tables = layout.pattern_tables();
    let words: usize = tables.iter().map(Vec::len).sum();
    Raw {
        tiled_bytes,
        mask_at,
        tables_at,
        bytes: tables_at + 4 * words as u64,
    }
}

/// Writes `layout`'s pattern tables and an all-held mask into `data`.
///
/// # Safety
/// `data` points to a mapped buffer of at least `raw.bytes` bytes.
pub(crate) unsafe fn write_tables(layout: &Layout, raw: &Raw, data: *mut u8) {
    let mut at = raw.tables_at as usize;
    let tables = layout.pattern_tables();
    // SAFETY: caller guarantees `data` holds `raw.bytes`.
    unsafe {
        write_mask(raw, data, u32::MAX);
        for table in &tables {
            for &v in table {
                data.add(at).cast::<u32>().write_unaligned(v);
                at += 4;
            }
        }
    }
}

/// Writes the held-levels bitmask (bit `m` = mip `m` is held; mips >= 32 held).
///
/// # Safety
/// As [`write_tables`].
pub(crate) unsafe fn write_mask(raw: &Raw, data: *mut u8, held: u32) {
    // SAFETY: caller guarantees `data` holds `raw.bytes`.
    unsafe { data.add(raw.mask_at as usize).cast::<u32>().write_unaligned(held) };
}

/// One subresource's compute dispatch: root constants and thread-group counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Job {
    pub(crate) constants: [u32; CONSTANTS],
    pub(crate) groups: [u32; 3],
}

/// Builds the dispatches for every subresource of `layout` (starting at title
/// mip `dropped` for mip-dropped textures); returns `None` if any subresource
/// cannot be detiled on the GPU (the caller falls back to CPU detiling).
pub(crate) fn jobs(
    layout: &Layout,
    footprints: &[D3D12_PLACED_SUBRESOURCE_FOOTPRINT],
    rows: &[u32],
    mips: u32,
    dropped: u32,
    raw: &Raw,
) -> Option<Vec<Job>> {
    let tables_at = u32::try_from(raw.tables_at).ok()?;
    let mask_at = u32::try_from(raw.mask_at).ok()?;
    let tables_vec = layout.pattern_tables();
    let tables = [&tables_vec[0][..], &tables_vec[1][..], &tables_vec[2][..]];
    let mut out = Vec::with_capacity(footprints.len());
    for (i, (footprint, &row_count)) in footprints.iter().zip(rows).enumerate() {
        let (host_mip, slice) = (i as u32 % mips, i as u32 / mips);
        let mip = host_mip + dropped;
        let p = layout.detile_params(mip, slice)?;
        // Guard against out-of-bounds reads when `read_size < layout.size` (mip-dropped).
        if (raw.tiled_bytes as u64) < layout.size && p.width > 0 && p.height > 0 && p.depth > 0 {
            let max_end = p
                .address(tables, p.width - 1, p.height - 1, p.depth - 1)
                .saturating_add(u64::from(p.element_bytes));
            if max_end > raw.tiled_bytes as u64 {
                return None;
            }
        }
        let f = footprint.Footprint;
        let bytes = u64::from(p.width) * u64::from(p.element_bytes);
        let depth_pitch = u64::from(f.RowPitch) * u64::from(row_count);
        let end = footprint.Offset + depth_pitch * u64::from(f.Depth);
        if bytes > u64::from(f.RowPitch)
            || p.height > row_count
            || p.depth > f.Depth
            || end > u64::from(u32::MAX)
            || p.height > 65535
            || p.depth > 65535
        {
            return None;
        }
        let words = bytes.div_ceil(4) as u32;
        out.push(Job {
            constants: [
                u32::from(p.linear),
                p.element_bytes,
                p.width,
                p.height,
                p.depth,
                p.first,
                p.slab,
                p.slice_size,
                p.mip_offset,
                p.pitch,
                p.block_w,
                p.block_h,
                p.block_log2,
                p.tail_x,
                p.tail_y,
                tables_at,
                footprint.Offset as u32,
                f.RowPitch,
                depth_pitch as u32,
                words,
                mask_at,
                mip,
            ],
            groups: [words.div_ceil(64), p.height, p.depth],
        });
    }
    Some(out)
}

/// Records the compute detiling of `raw` (upload heap) into `linear` (default
/// heap, starts and ends in `COMMON`).
pub(crate) fn record(
    list: &ID3D12GraphicsCommandList,
    d: &Detiler,
    raw: &ID3D12Resource,
    linear: &ID3D12Resource,
    jobs: &[Job],
) {
    use crate::host::transition;
    // SAFETY: open list; live buffers kept by the caller; detiler's own root signature.
    unsafe {
        list.ResourceBarrier(&[transition(
            linear,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        )]);
        list.SetComputeRootSignature(&d.root);
        list.SetPipelineState(&d.pso);
        list.SetComputeRootShaderResourceView(1, raw.GetGPUVirtualAddress());
        list.SetComputeRootUnorderedAccessView(2, linear.GetGPUVirtualAddress());
        for job in jobs {
            list.SetComputeRoot32BitConstants(0, CONSTANTS as u32, job.constants.as_ptr().cast(), 0);
            list.Dispatch(job.groups[0], job.groups[1], job.groups[2]);
        }
        list.ResourceBarrier(&[transition(
            linear,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_COMMON,
        )]);
    }
}

/// Diagnostics since the last frame-rate line: textures detiled on the card,
/// dispatches, tiled bytes copied, copy microseconds, stagings made for card, refused.
pub(crate) static STATS: [std::sync::atomic::AtomicU64; 6] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 6];

#[cfg(test)]
mod tests {
    use super::*;
    use victory_formats::texture::{Dimension, Swizzle, Texture, TextureDesc};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    struct Rig {
        device: ID3D12Device,
        queue: ID3D12CommandQueue,
        allocator: ID3D12CommandAllocator,
        list: ID3D12GraphicsCommandList,
        fence: ID3D12Fence,
        event: HANDLE,
        submitted: u64,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            // SAFETY: our own event.
            unsafe {
                let _ = CloseHandle(self.event);
            }
        }
    }

    impl Rig {
        fn new() -> Option<Rig> {
            // SAFETY: plain object creation on a software adapter device.
            unsafe {
                let adapter = crate::host::warp_adapter().ok()?;
                let mut device: Option<ID3D12Device> = None;
                D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device).ok()?;
                let device = device?;
                let queue: ID3D12CommandQueue = device
                    .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                        Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                        ..Default::default()
                    })
                    .ok()?;
                let allocator: ID3D12CommandAllocator = device
                    .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                    .ok()?;
                let list: ID3D12GraphicsCommandList = device
                    .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
                    .ok()?;
                let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
                let event = CreateEventW(None, false, false, None).ok()?;
                Some(Rig {
                    device,
                    queue,
                    allocator,
                    list,
                    fence,
                    event,
                    submitted: 0,
                })
            }
        }

        fn run(&mut self, record: impl FnOnce(&ID3D12GraphicsCommandList)) {
            record(&self.list);
            self.submitted += 1;
            // SAFETY: our list, queue, and fence; reset only after completion.
            unsafe {
                self.list.Close().unwrap();
                let run: ID3D12CommandList = windows::core::Interface::cast(&self.list).unwrap();
                self.queue.ExecuteCommandLists(&[Some(run)]);
                self.queue.Signal(&self.fence, self.submitted).unwrap();
                self.fence.SetEventOnCompletion(self.submitted, self.event).unwrap();
                assert_eq!(WaitForSingleObject(self.event, 60_000), WAIT_OBJECT_0);
                let removed = self.device.GetDeviceRemovedReason();
                assert!(removed.is_ok(), "device lost: {removed:?}");
                self.allocator.Reset().unwrap();
                self.list.Reset(&self.allocator, None).unwrap();
            }
        }

        fn buffer(
            &self,
            kind: D3D12_HEAP_TYPE,
            flags: D3D12_RESOURCE_FLAGS,
            state: D3D12_RESOURCE_STATES,
            bytes: u64,
        ) -> ID3D12Resource {
            let heap = D3D12_HEAP_PROPERTIES {
                Type: kind,
                ..Default::default()
            };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: bytes.max(1),
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                Flags: flags,
                ..Default::default()
            };
            let mut out: Option<ID3D12Resource> = None;
            // SAFETY: plain buffer creation.
            unsafe {
                self.device
                    .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut out)
                    .unwrap();
            }
            out.unwrap()
        }
    }

    fn map(buffer: &ID3D12Resource) -> *mut u8 {
        let mut data = std::ptr::null_mut();
        // SAFETY: CPU-visible buffer of our own.
        unsafe { buffer.Map(0, None, Some(&mut data)).unwrap() };
        data.cast()
    }

    struct Case {
        dimension: Dimension,
        width: u32,
        height: u32,
        depth: u32,
        mips: u32,
        format: DXGI_FORMAT,
        swizzle: Swizzle,
    }

    fn both(rig: &mut Rig, d: &Detiler, case: &Case, held: u32, dropped: u32) -> bool {
        let texture = Texture::new(&TextureDesc {
            dimension: case.dimension,
            width: case.width,
            height: case.height,
            depth_or_array: case.depth,
            mips: case.mips,
            format: case.format.0 as u32,
            samples: 1,
            swizzle: case.swizzle,
        })
        .unwrap();
        let layout = texture.planes[0].layout.clone();
        let three_d = case.dimension == Dimension::Tex3d;
        let host_mips = case.mips - dropped;
        let host_w = (case.width >> dropped).max(1);
        let host_h = (case.height >> dropped).max(1);
        let host_desc = D3D12_RESOURCE_DESC {
            Dimension: if three_d {
                D3D12_RESOURCE_DIMENSION_TEXTURE3D
            } else {
                D3D12_RESOURCE_DIMENSION_TEXTURE2D
            },
            Width: u64::from(host_w),
            Height: host_h,
            DepthOrArraySize: case.depth as u16,
            MipLevels: host_mips as u16,
            Format: case.format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..Default::default()
        };
        let count = if three_d { host_mips } else { host_mips * case.depth };
        let mut footprints = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); count as usize];
        let mut rows = vec![0u32; count as usize];
        let mut total = 0u64;
        // SAFETY: outputs for `count` subresources.
        unsafe {
            rig.device.GetCopyableFootprints(
                &host_desc,
                0,
                count,
                0,
                Some(footprints.as_mut_ptr()),
                Some(rows.as_mut_ptr()),
                None,
                Some(&mut total),
            );
        }
        let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ u64::from(case.width * 31 + case.height);
        let read_size = layout.bytes_from(dropped) as usize;
        let tiled: Vec<u8> = (0..read_size)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed >> 24) as u8
            })
            .collect();
        let mut cpu = vec![0xA5u8; total as usize + 4096];
        for (i, (footprint, &r)) in footprints.iter().zip(&rows).enumerate() {
            let (host_mip, slice) = (i as u32 % host_mips, i as u32 / host_mips);
            let mip = host_mip + dropped;
            let f = footprint.Footprint;
            let pitch = f.RowPitch as usize;
            let len = pitch * r as usize * f.Depth as usize;
            let linear = &mut cpu[footprint.Offset as usize..][..len];
            if mip < 32 && held & (1 << mip) == 0 {
                linear.fill(0);
                continue;
            }
            layout
                .detile(&tiled, mip, slice, linear, pitch, pitch * r as usize)
                .unwrap();
        }
        let raw = raw_layout(&layout, read_size);
        let jobs = jobs(&layout, &footprints, &rows, host_mips, dropped, &raw)
            .expect("detilable on the card");
        let upload = rig.buffer(
            D3D12_HEAP_TYPE_UPLOAD,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            raw.bytes,
        );
        let data = map(&upload);
        // SAFETY: mapping holds `raw.bytes`.
        unsafe {
            std::ptr::copy_nonoverlapping(tiled.as_ptr(), data, tiled.len());
            write_tables(&layout, &raw, data);
            write_mask(&raw, data, held);
        }
        let linear = rig.buffer(
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_COMMON,
            total,
        );
        let back = rig.buffer(
            D3D12_HEAP_TYPE_READBACK,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_COPY_DEST,
            total,
        );
        rig.run(|list| {
            record(list, d, &upload, &linear, &jobs);
            // SAFETY: open list; buffers outlive execution.
            unsafe { list.CopyBufferRegion(&back, 0, &linear, 0, total) };
        });
        let read = map(&back);
        // SAFETY: read-back buffer holds `total` bytes.
        let card = unsafe { std::slice::from_raw_parts(read.cast_const(), total as usize) };
        let mut same = true;
        for (i, (footprint, &r)) in footprints.iter().zip(&rows).enumerate() {
            let mip = (i as u32 % host_mips) + dropped;
            let m = &layout.mips[mip as usize];
            let n = layout.desc.element_bytes as usize;
            let f = footprint.Footprint;
            for z in 0..m.depth as usize {
                for y in 0..m.height as usize {
                    let at = footprint.Offset as usize
                        + z * f.RowPitch as usize * r as usize
                        + y * f.RowPitch as usize;
                    let own = m.width as usize * n;
                    if cpu[at..at + own] != card[at..at + own] {
                        if same {
                            eprintln!(
                                "{:?} {}x{}x{} {:?} dropped={dropped}: sub {i} (mip {mip}) row {y} z {z} differs",
                                case.swizzle, case.width, case.height, case.depth, case.format
                            );
                        }
                        same = false;
                    }
                }
            }
        }
        same
    }

    #[test]
    fn the_card_detiles_as_the_cpu_does() {
        let Some(mut rig) = Rig::new() else {
            return;
        };
        let d = make(&rig.device).expect("the detile shader compiles");
        let t2 = |width, height, mips, format, swizzle| Case {
            dimension: Dimension::Tex2d,
            width,
            height,
            depth: 1,
            mips,
            format,
            swizzle,
        };
        let cases = [
            t2(1024, 1024, 11, DXGI_FORMAT_BC7_UNORM, Swizzle::SW_64KB_S),
            t2(512, 256, 10, DXGI_FORMAT_BC1_UNORM, Swizzle::SW_64KB_S),
            t2(256, 256, 9, DXGI_FORMAT_BC3_UNORM, Swizzle::SW_4KB_S),
            t2(100, 60, 7, DXGI_FORMAT_BC5_UNORM, Swizzle::SW_4KB_S),
            t2(300, 200, 9, DXGI_FORMAT_R8G8B8A8_UNORM, Swizzle::SW_64KB_S),
            t2(129, 65, 8, DXGI_FORMAT_R8G8B8A8_UNORM, Swizzle::SW_4KB_S),
            t2(97, 61, 7, DXGI_FORMAT_R8_UNORM, Swizzle::SW_64KB_S),
            t2(77, 33, 7, DXGI_FORMAT_R16_UNORM, Swizzle::SW_4KB_D),
            t2(64, 64, 7, DXGI_FORMAT_R16G16B16A16_FLOAT, Swizzle::SW_64KB_D),
            t2(50, 30, 6, DXGI_FORMAT_R32G32B32A32_FLOAT, Swizzle::SW_256B_S),
            t2(33, 17, 6, DXGI_FORMAT_R8_UNORM, Swizzle::SW_256B_D),
            t2(130, 70, 8, DXGI_FORMAT_R8G8B8A8_UNORM, Swizzle::LINEAR),
            t2(61, 3, 6, DXGI_FORMAT_R8_UNORM, Swizzle::LINEAR),
            Case { depth: 6, ..t2(128, 128, 8, DXGI_FORMAT_BC1_UNORM, Swizzle::SW_4KB_S) },
            Case { depth: 3, ..t2(70, 70, 4, DXGI_FORMAT_R8G8B8A8_UNORM, Swizzle::SW_64KB_S) },
            Case { dimension: Dimension::Tex3d, depth: 16, ..t2(64, 32, 7, DXGI_FORMAT_R8G8B8A8_UNORM, Swizzle::SW_4KB_S) },
            Case { dimension: Dimension::Tex3d, depth: 40, ..t2(33, 20, 6, DXGI_FORMAT_R16_UNORM, Swizzle::SW_64KB_S) },
        ];
        let mut all = true;
        for case in &cases {
            all &= both(&mut rig, &d, case, u32::MAX, 0);
        }
        // Levels on demand (dropped = 0):
        all &= both(&mut rig, &d, &cases[0], !0b11, 0);
        all &= both(&mut rig, &d, &cases[4], !0b1, 0);
        // Mip-dropped textures (e.g. VICTORY_TEXTURE_MAX_DIM=1024 on 8 GB VRAM):
        all &= both(&mut rig, &d, &cases[0], u32::MAX, 1);
        all &= both(&mut rig, &d, &cases[1], u32::MAX, 2);
        assert!(all, "the card's detiling differs from the CPU's");
    }
}

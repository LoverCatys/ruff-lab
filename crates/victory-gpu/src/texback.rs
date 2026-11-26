//! Textures shaders write that the title's own code then reads on the CPU.
//! On the console the CPU and the GPU share memory, so what a compute pass
//! stores into a texture is in title memory for the CPU to read. Here it stays
//! in the texture's host copy. The title's ocean is one: its wave passes write
//! a 256x256 height map (R32_FLOAT) and a displacement map (R32G32_FLOAT), and
//! its CPU code samples both, which is what boats float on; it read memory
//! nothing had written (run rw1, `VICTORY_READ_WATCH=1`: about 30 places in
//! the title read those two kinds), and boats sank.
//!
//! For textures of the kinds in `VICTORY_TEXTURE_BACK` (comma-separated
//! `<width>x<height>f<format>`; `0` for none; by default the kinds the read
//! watch found the title reading), a list in which shaders write one copies it
//! into a read-back buffer of ours when it closes, and once the list has run
//! the texels are tiled into the texture's title memory in the console's
//! layout, as the console's GPU would have left them.
//!
//! Read-back buffers up to 16 MiB are pooled by power-of-two size
//! (`VICTORY_READBACK_POOL`, on by default; `kitty-core/Kitty6` `43c2f3f`), and
//! tiling/writing into title memory runs on a dedicated writer thread so
//! `watch::title_memory_reading` waits during world streaming never stall
//! submission landing (`VICTORY_TEXBACK_ASYNC`, on by default; `kitty-core/Kitty6`
//! `eeb6e35`).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;

use crate::object::This;

/// The kinds copied back: (width, height, format).
fn kinds() -> &'static [(u64, u32, u32)] {
    static KINDS: OnceLock<Vec<(u64, u32, u32)>> = OnceLock::new();
    KINDS.get_or_init(|| {
        const DEFAULT: &str = "256x256f41,256x256f16,64x64f16,128x128f36";
        let list = std::env::var("VICTORY_TEXTURE_BACK").unwrap_or_else(|_| DEFAULT.to_owned());
        let kinds: Vec<(u64, u32, u32)> = list
            .split(',')
            .filter_map(|k| {
                let (size, format) = k.trim().split_once('f')?;
                let (w, h) = size.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?, format.parse().ok()?))
            })
            .collect();
        if !kinds.is_empty() {
            tracing::info!(
                ?kinds,
                "textures of these kinds that shaders write are copied back into title memory (VICTORY_TEXTURE_BACK)"
            );
        }
        kinds
    })
}

/// Whether title texture `resource` is of a kind copied back.
pub(crate) fn wanted(resource: This) -> bool {
    let kinds = kinds();
    !kinds.is_empty()
        && crate::resource::texture_kind(resource).is_some_and(|k| kinds.contains(&k))
}

/// A read-back buffer nothing holds.
struct Spare(ID3D12Resource);

// SAFETY: a D3D12 buffer (free-threaded), touched by nobody while spare.
unsafe impl Send for Spare {}

/// Spare read-back buffers by power-of-two size, and their total bytes.
static SPARE: Mutex<(BTreeMap<u64, Vec<Spare>>, u64)> = Mutex::new((BTreeMap::new(), 0));
/// Default spare cap (128 MiB for 16 GB RAM PCs; configurable via
/// `VICTORY_READBACK_POOL_MB`), and the largest buffer pooled (16 MiB).
const SPARE_MOST: (u64, u64) = (128 << 20, 16 << 20);
/// Read-back buffers reused from the pool, and newly created, since the start.
static SPARE_STATS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Whether read-back buffers are pooled by power-of-two size (on unless
/// `VICTORY_READBACK_POOL=0`).
fn pool_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_READBACK_POOL").is_none_or(|v| v != "0"))
}

/// Maximum spare read-back pool bytes (`VICTORY_READBACK_POOL_MB`, default 128).
fn spare_most() -> u64 {
    static MOST: OnceLock<u64> = OnceLock::new();
    *MOST.get_or_init(|| {
        std::env::var("VICTORY_READBACK_POOL_MB")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map_or(SPARE_MOST.0, |mb| mb << 20)
    })
}

/// The power-of-two size a buffer of `total` bytes is pooled at; 0: not pooled.
fn pool_size(total: u64, on: bool) -> u64 {
    let size = total.max(64 << 10).next_power_of_two();
    if on && size <= SPARE_MOST.1 { size } else { 0 }
}

/// A read-back buffer of at least `total` bytes, and its pool size (0: not pooled).
fn read_back_buffer(host: &crate::host::Host, total: u64) -> Option<(ID3D12Resource, u64)> {
    let size = pool_size(total, pool_on());
    if size != 0 {
        let spare = {
            let mut all = SPARE.lock().unwrap_or_else(|e| e.into_inner());
            let taken = all.0.get_mut(&size).and_then(Vec::pop);
            if taken.is_some() {
                all.1 -= size;
            }
            taken
        };
        if let Some(Spare(buffer)) = spare {
            SPARE_STATS[0].fetch_add(1, Ordering::Relaxed);
            return Some((buffer, size));
        }
    }
    let made = if size != 0 { size } else { total };
    let buffer = host.read_back_buffer(made)?;
    SPARE_STATS[1].fetch_add(1, Ordering::Relaxed);
    crate::host::name(&buffer, || format!("texback read-back buffer of {made} bytes"));
    Some((buffer, size))
}

/// A texture's copy into a read-back buffer, recorded into a list, and where
/// its texels go once the list has run.
pub(crate) struct Back {
    buffer: ID3D12Resource,
    /// Pool size bucket from [`read_back_buffer`]; 0 means release on drop.
    pooled: u64,
    footprints: Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>,
    rows: Vec<u32>,
    mips: u32,
    layout: victory_formats::texture::Layout,
    /// The texture's title memory and its layout's bytes.
    at: usize,
    size: usize,
}

impl Drop for Back {
    fn drop(&mut self) {
        if self.pooled != 0 {
            let mut all = SPARE.lock().unwrap_or_else(|e| e.into_inner());
            if all.1 + self.pooled <= spare_most() {
                all.1 += self.pooled;
                all.0
                    .entry(self.pooled)
                    .or_default()
                    .push(Spare(self.buffer.clone()));
            }
        }
    }
}

/// Records into `list` the copy of `texture` (title resource `resource`'s
/// host copy, in the copy-source state) into a read-back buffer.
pub(crate) fn record(
    list: &ID3D12GraphicsCommandList,
    resource: This,
    texture: &ID3D12Resource,
) -> Option<Back> {
    let host = crate::host::host()?;
    let (layout, at, size) = crate::resource::title_texture(resource)?;
    // SAFETY: a live texture.
    let desc = unsafe { texture.GetDesc() };
    let mips = u32::from(desc.MipLevels.max(1));
    let subresources = mips * u32::from(desc.DepthOrArraySize.max(1));
    let n = subresources as usize;
    let mut footprints = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n];
    let mut rows = vec![0u32; n];
    let mut total = 0u64;
    // SAFETY: out arrays of `n` entries.
    unsafe {
        host.device.GetCopyableFootprints(
            &desc,
            0,
            subresources,
            0,
            Some(footprints.as_mut_ptr()),
            Some(rows.as_mut_ptr()),
            None,
            Some(&mut total),
        )
    };
    let (buffer, pooled) = read_back_buffer(host, total)?;
    for (i, footprint) in footprints.iter().enumerate() {
        let to = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: a borrowed pointer the location never releases.
            pResource: unsafe { std::mem::transmute_copy(&buffer) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: *footprint,
            },
        };
        let from = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: as above.
            pResource: unsafe { std::mem::transmute_copy(texture) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: i as u32,
            },
        };
        // SAFETY: an open list; the texture is in the copy-source state.
        unsafe { list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
    }
    if COPIED.fetch_add(1, Ordering::Relaxed) == 0 {
        tracing::info!(
            width = desc.Width,
            height = desc.Height,
            format = desc.Format.0,
            swizzle = layout.desc.swizzle.0,
            at = format_args!("{at:#x}"),
            "a texture shaders wrote is copied back into title memory, the first (VICTORY_TEXTURE_BACK)"
        );
    }
    Some(Back {
        buffer,
        pooled,
        footprints,
        rows,
        mips,
        layout,
        at,
        size,
    })
}

/// Textures copied back since the start.
pub(crate) static COPIED: AtomicU64 = AtomicU64::new(0);

/// Whether texture read-backs are tiled and written into title memory on a
/// dedicated background thread (`VICTORY_TEXBACK_ASYNC`, on unless `0`).
fn asynchronous() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_TEXBACK_ASYNC").is_none_or(|v| v != "0"))
}

/// Linear texels copied out of a read-back buffer, ready to tile into title memory.
struct Landing {
    footprints: Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>,
    rows: Vec<u32>,
    mips: u32,
    layout: victory_formats::texture::Layout,
    at: usize,
    size: usize,
    data: Vec<u8>,
}

/// Once the list that recorded `back` has run: copies linear texels out of
/// the read-back buffer and hands off tiling/writing to [`writer`] (or runs
/// inline when `VICTORY_TEXBACK_ASYNC=0`).
pub(crate) fn land(back: &Back) {
    let len = back
        .footprints
        .iter()
        .zip(&back.rows)
        .map(|(f, &rows)| {
            f.Offset as usize
                + f.Footprint.RowPitch as usize * rows as usize * f.Footprint.Depth as usize
        })
        .max()
        .unwrap_or(0);
    let mut mapped = std::ptr::null_mut();
    // SAFETY: our read-back buffer, written by the list that has run; `len`
    // ends with its last subresource.
    let data = unsafe {
        if back.buffer.Map(0, None, Some(&mut mapped)).is_err() || mapped.is_null() {
            return;
        }
        let data = std::slice::from_raw_parts(mapped.cast::<u8>().cast_const(), len).to_vec();
        back.buffer.Unmap(0, None);
        data
    };
    let landing = Landing {
        footprints: back.footprints.clone(),
        rows: back.rows.clone(),
        mips: back.mips,
        layout: back.layout.clone(),
        at: back.at,
        size: back.size,
        data,
    };
    match asynchronous().then(writer).flatten() {
        Some(writer) => writer.send(landing),
        None => write(&landing),
    }
}

/// Tiles a read-back's texels into its title allocation under the mapping read lock.
fn write(landing: &Landing) {
    static LANDED: AtomicU64 = AtomicU64::new(0);
    let _mapping = crate::watch::title_memory_reading(landing.at, landing.size);
    let mut tiled = vec![0u8; landing.size];
    // Keep title bytes the subresources do not cover; abort if unmapped.
    if !crate::host::read_into(landing.at as *const u8, &mut tiled) {
        return;
    }
    for (i, (footprint, &rows)) in landing.footprints.iter().zip(&landing.rows).enumerate() {
        let (mip, slice) = (i as u32 % landing.mips, i as u32 / landing.mips);
        let f = footprint.Footprint;
        let pitch = f.RowPitch as usize;
        let from = footprint.Offset as usize;
        let end = from + pitch * rows as usize * f.Depth as usize;
        if end > landing.data.len() {
            return;
        }
        let linear = &landing.data[from..end];
        if let Err(e) = landing
            .layout
            .tile(&mut tiled, mip, slice, linear, pitch, pitch * rows as usize)
        {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                tracing::warn!(
                    ?e,
                    mip,
                    slice,
                    swizzle = landing.layout.desc.swizzle.0,
                    "a texture copied back could not be tiled"
                )
            });
            return;
        }
    }
    crate::host::copy_mapped(landing.at as *mut u8, tiled.as_ptr(), landing.size, true);
    let landed = LANDED.fetch_add(1, Ordering::Relaxed) + 1;
    if landed == 1 || landed.is_multiple_of(128) {
        tracing::info!(
            landed,
            bytes = landing.size,
            buffers_reused = SPARE_STATS[0].load(Ordering::Relaxed),
            buffers_made = SPARE_STATS[1].load(Ordering::Relaxed),
            "texture readback landed"
        );
    }
}

/// Background thread that tiles and writes texture read-backs into title memory,
/// coalescing multiple pending landings for the same allocation to the newest.
struct Writer {
    waiting: Mutex<HashMap<usize, Landing>>,
    more: Condvar,
}

impl Writer {
    fn send(&self, landing: Landing) {
        let mut waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
        waiting.insert(landing.at, landing);
        drop(waiting);
        self.more.notify_one();
    }
}

fn writer() -> Option<&'static Writer> {
    static WRITER: OnceLock<Option<&'static Writer>> = OnceLock::new();
    *WRITER.get_or_init(|| {
        let writer: &'static Writer = Box::leak(Box::new(Writer {
            waiting: Mutex::new(HashMap::new()),
            more: Condvar::new(),
        }));
        let started = std::thread::Builder::new()
            .name("texture read-backs".into())
            .spawn(move || loop {
                let batch: Vec<Landing> = {
                    let mut waiting = writer.waiting.lock().unwrap_or_else(|e| e.into_inner());
                    while waiting.is_empty() {
                        waiting = writer.more.wait(waiting).unwrap_or_else(|e| e.into_inner());
                    }
                    waiting.drain().map(|(_, landing)| landing).collect()
                };
                for landing in &batch {
                    write(landing);
                }
            });
        match started {
            Ok(_) => Some(writer),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "no thread for texture read-backs: they are written where they land"
                );
                None
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::pool_size;

    #[test]
    fn readback_buffers_pool_by_power_of_two_up_to_16_mib() {
        assert_eq!(pool_size(1, true), 64 << 10);
        assert_eq!(pool_size(64 << 10, true), 64 << 10);
        assert_eq!(pool_size((256 * 256 * 4) * 4 / 3, true), 512 << 10);
        assert_eq!(pool_size(16 << 20, true), 16 << 20);
        assert_eq!(pool_size((16 << 20) + 1, true), 0);
        assert_eq!(pool_size(1, false), 0);
    }
}

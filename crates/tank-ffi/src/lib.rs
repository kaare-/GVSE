#![allow(clippy::not_unsafe_ptr_arg_deref)]
//! C ABI described in `include/tank.h`.
//!
//! The pointer returned by [`tank_pixels`] is valid until the next
//! `tank_pixels`, `tank_free`, or `tank_load` on that handle. The core
//! is single-threaded; the shell calls it from one queue.

use std::ptr;

use tank_core::{glyph_bits, Inspect, LineageCard, SpeciesNode, Specimen, Tank, TankInput, TelEvent, ToolEvent};

pub struct TankHandle {
    tank: Tank,
    rgba: Vec<u8>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ToolEventC {
    pub kind: u8,
    pub material: u8,
    pub x: u16,
    pub y: u16,
    pub r: u16,
    pub variant: u8,
    pub _pad: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TankInputC {
    pub grav_x: f32,
    pub grav_y: f32,
    pub paused: u8,
    pub always_day: u8,
    pub swirl: u8,
    pub _pad1: u8,
    pub light_slider: f32,
    pub t_base: f32,
    pub flash_x: i32,
    pub flash_y: i32,
    pub flash_r: i32,
    pub tools: *const ToolEventC,
    pub tool_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InspectRec {
    pub hit: u8,
    pub material: u8,
    pub sat: u8,
    pub moisture: u8,
    pub variant: u8,
    pub _pad0: u8,
    pub _pad1: u8,
    pub _pad2: u8,
    pub owner: u32,
    pub vx: u32,
    pub vy: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EventRec {
    pub kind: u32,
    pub arg0: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TelRec {
    pub kind: u32,
    pub arg0: u32,
}

/// Opaque scoop payload (Phase 3): what `tank_scoop` lifted. Hand it to
/// `tank_release` (back into the tank) or `tank_discard` (out of the
/// tank); both consume it. `tank_carried_free` only frees the memory —
/// the organisms inside are then lost without their counts settling.
pub struct Carried {
    inner: tank_core::Carried,
}

/// Living-cell inspect record (Phase 3). `cell == UINT32_MAX` means no
/// living cell under the pixel; the bbox is the organism's tracking
/// rectangle in pixels (inclusive).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CellInspectRec {
    pub cell: u32,
    pub org: u32,
    pub genotype: u32,
    pub species: u16,
    pub mode: u8,
    pub energy_frac: u8,
    pub generation: u32,
    pub org_cells: u32,
    pub glued_kin: u32,
    pub age_ticks: u32,
    pub max_age_ticks: u32,
    pub children: u32,
    pub stress: u8,
    pub _pad0: u8,
    pub _pad1: u8,
    pub _pad2: u8,
    pub bbox_x0: u16,
    pub bbox_y0: u16,
    pub bbox_x1: u16,
    pub bbox_y1: u16,
}

/// Lineage card numbers (Phase 3); names via `tank_species_name`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LineageRec {
    pub cell: u32,
    pub org: u32,
    pub genotype: u32,
    pub species: u16,
    pub parent_species: u16,
    pub generation: u32,
    pub mutations_since_seed: u16,
    pub species_depth: u16,
    pub kin_alive: u32,
    pub descendants_alive: u32,
    pub archived_relatives: u32,
    pub children: u32,
}

/// One species-tree node for the tree-of-life view.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpeciesNodeRec {
    pub id: u16,
    pub parent: u16,
    pub depth: u16,
    pub archived: u8,
    pub extinct: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub _pad: u8,
    pub live_cells: u32,
    pub live_genotypes: u32,
    pub forms: u32,
    pub born: u32,
    pub first_tick: u64,
}

/// Numbers of an archived specimen (strings via `tank_specimen_text`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpecimenInfoRec {
    pub id: u64,
    pub tank_seed: u64,
    pub tick: u64,
    pub archive_parent: u64,
    pub has_archive_parent: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub species_depth: u16,
    pub mutations_since_seed: u16,
    pub generation: u32,
    pub cells: u32,
    pub slots: u32,
    pub age_ticks: u32,
    pub max_age_ticks: u32,
    pub children: u32,
    pub kin_alive: u32,
    pub descendants_alive: u32,
    pub portrait_w: u16,
    pub portrait_h: u16,
}

#[no_mangle]
pub extern "C" fn tank_new(seed: u64, w_cells: u32, h_cells: u32) -> *mut TankHandle {
    match Tank::try_new(seed, w_cells, h_cells) {
        Ok(tank) => Box::into_raw(Box::new(TankHandle {
            tank,
            rgba: Vec::new(),
        })),
        Err(_) => ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn tank_preset(seed: u64) -> *mut TankHandle {
    Box::into_raw(Box::new(TankHandle {
        tank: Tank::preset(seed),
        rgba: Vec::new(),
    }))
}

#[no_mangle]
pub extern "C" fn tank_empty(seed: u64) -> *mut TankHandle {
    Box::into_raw(Box::new(TankHandle {
        tank: Tank::empty(seed),
        rgba: Vec::new(),
    }))
}

#[no_mangle]
pub extern "C" fn tank_seed_algae(handle: *mut TankHandle, px: u32, py: u32) -> u32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return u32::MAX;
    };
    handle
        .tank
        .seed_algae(px as u16, py as u16)
        .unwrap_or(u32::MAX)
}

#[no_mangle]
pub extern "C" fn tank_live_cells(handle: *const TankHandle) -> u32 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.live_cells() as u32)
        .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn tank_swipe_film(
    handle: *mut TankHandle,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    radius: i32,
) -> u32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return 0;
    };
    handle.tank.swipe_film(x0, y0, x1, y1, radius)
}

#[no_mangle]
pub extern "C" fn tank_free(handle: *mut TankHandle) {
    if handle.is_null() {
        return;
    }
    unsafe { drop(Box::from_raw(handle)) };
}

#[no_mangle]
pub extern "C" fn tank_tick(handle: *mut TankHandle, input: *const TankInputC) {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return;
    };
    let Some(input) = (unsafe { input.as_ref() }) else {
        return;
    };
    let mut tools = Vec::new();
    if !input.tools.is_null() && input.tool_count > 0 {
        let n = (input.tool_count as usize).min(1024);
        let slice = unsafe { std::slice::from_raw_parts(input.tools, n) };
        tools.extend(slice.iter().map(|t| ToolEvent {
            kind: t.kind,
            material: t.material,
            x: t.x,
            y: t.y,
            r: t.r,
            variant: t.variant,
        }));
    }
    handle.tank.tick(&TankInput {
        grav_x: input.grav_x,
        grav_y: input.grav_y,
        paused: input.paused != 0,
        always_day: input.always_day != 0,
        swirl: input.swirl != 0,
        light_slider: input.light_slider,
        t_base: input.t_base,
        flash_x: input.flash_x,
        flash_y: input.flash_y,
        flash_r: input.flash_r,
        tools,
    });
}

#[no_mangle]
pub extern "C" fn tank_undo(handle: *mut TankHandle) -> u8 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return 0;
    };
    u8::from(handle.tank.undo())
}

#[no_mangle]
pub extern "C" fn tank_undo_depth(handle: *const TankHandle) -> u32 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.undo_depth() as u32)
        .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn tank_pixels(handle: *mut TankHandle, w: *mut u32, h: *mut u32) -> *const u8 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ptr::null();
    };
    let (pw, ph) = handle.tank.pixel_size();
    let TankHandle { tank, rgba } = handle;
    tank.write_rgba(rgba);
    if !w.is_null() {
        unsafe { *w = pw };
    }
    if !h.is_null() {
        unsafe { *h = ph };
    }
    handle.rgba.as_ptr()
}

#[no_mangle]
pub extern "C" fn tank_pixel_hash(handle: *mut TankHandle) -> u64 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return 0;
    };
    handle.tank.pixel_hash()
}

#[no_mangle]
pub extern "C" fn tank_inspect(handle: *const TankHandle, px: u32, py: u32) -> InspectRec {
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return InspectRec::empty();
    };
    InspectRec::from(handle.tank.inspect(px, py))
}

#[no_mangle]
pub extern "C" fn tank_poll_event(handle: *mut TankHandle) -> EventRec {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return EventRec { kind: 0, arg0: 0 };
    };
    match handle.tank.poll_event() {
        Some(ev) => EventRec {
            kind: ev.kind,
            arg0: ev.arg0,
        },
        None => EventRec { kind: 0, arg0: 0 },
    }
}

/// Lift everything inside the O (pixel centre, pixel radius). Null when
/// the bucket would be empty.
#[no_mangle]
pub extern "C" fn tank_scoop(handle: *mut TankHandle, cx: i32, cy: i32, r: i32) -> *mut Carried {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ptr::null_mut();
    };
    let inner = handle.tank.scoop_region(cx, cy, r);
    if inner.is_empty() {
        return ptr::null_mut();
    }
    Box::into_raw(Box::new(Carried { inner }))
}

/// Living cells in the bucket.
#[no_mangle]
pub extern "C" fn tank_carried_cells(carried: *const Carried) -> u32 {
    unsafe { carried.as_ref() }.map(|c| c.inner.cell_count()).unwrap_or(0)
}

/// Put the bucket down centred on pixel `(x, y)`. Returns 1 and consumes
/// `carried` on success; 0 (no room / out of bounds) leaves it valid.
#[no_mangle]
pub extern "C" fn tank_release(handle: *mut TankHandle, carried: *mut Carried, x: i32, y: i32) -> u8 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return 0;
    };
    let Some(c) = (unsafe { carried.as_mut() }) else {
        return 0;
    };
    let inner = std::mem::replace(&mut c.inner, tank_core::Carried::empty());
    match handle.tank.release(inner, x, y) {
        Ok(()) => {
            unsafe { drop(Box::from_raw(carried)) };
            1
        }
        Err((inner, _)) => {
            c.inner = inner;
            0
        }
    }
}

/// Throw the bucket away: the organisms leave the tank. Consumes `carried`.
#[no_mangle]
pub extern "C" fn tank_discard(handle: *mut TankHandle, carried: *mut Carried) {
    if carried.is_null() {
        return;
    }
    let boxed = unsafe { Box::from_raw(carried) };
    if let Some(handle) = unsafe { handle.as_mut() } {
        handle.tank.discard(boxed.inner);
    }
}

#[no_mangle]
pub extern "C" fn tank_carried_free(carried: *mut Carried) {
    if carried.is_null() {
        return;
    }
    unsafe { drop(Box::from_raw(carried)) };
}

/// Remove anything under the brush (physics-cell coords, bottom-left
/// origin). Returns living cells removed.
#[no_mangle]
pub extern "C" fn tank_remove_at(handle: *mut TankHandle, vx: i32, vy: i32, r: i32) -> u32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return 0;
    };
    handle.tank.remove_at(vx, vy, r)
}

#[no_mangle]
pub extern "C" fn tank_inspect_cell(handle: *const TankHandle, px: u32, py: u32) -> CellInspectRec {
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return CellInspectRec::empty();
    };
    let i = handle.tank.inspect(px, py);
    if i.cell == u32::MAX {
        return CellInspectRec::empty();
    }
    let bb = handle.tank.org_bbox(i.org).unwrap_or((0, 0, 0, 0));
    CellInspectRec {
        cell: i.cell,
        org: i.org,
        genotype: i.genotype,
        species: i.species,
        mode: i.mode,
        energy_frac: i.energy_frac,
        generation: i.generation,
        org_cells: i.org_cells,
        glued_kin: i.glued_kin,
        age_ticks: i.age_ticks,
        max_age_ticks: i.max_age_ticks,
        children: i.children,
        stress: i.stress,
        _pad0: 0,
        _pad1: 0,
        _pad2: 0,
        bbox_x0: bb.0,
        bbox_y0: bb.1,
        bbox_x1: bb.2,
        bbox_y1: bb.3,
    }
}

/// Tracking rectangle of a living organism (inclusive pixels). 0 = gone.
#[no_mangle]
pub extern "C" fn tank_org_bbox(
    handle: *const TankHandle,
    org: u32,
    x0: *mut u16,
    y0: *mut u16,
    x1: *mut u16,
    y1: *mut u16,
) -> u8 {
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return 0;
    };
    let Some(bb) = handle.tank.org_bbox(org) else {
        return 0;
    };
    unsafe {
        if !x0.is_null() {
            *x0 = bb.0;
        }
        if !y0.is_null() {
            *y0 = bb.1;
        }
        if !x1.is_null() {
            *x1 = bb.2;
        }
        if !y1.is_null() {
            *y1 = bb.3;
        }
    }
    1
}

#[no_mangle]
pub extern "C" fn tank_lineage(handle: *const TankHandle, cell: u32) -> LineageRec {
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return LineageRec::empty();
    };
    match handle.tank.lineage_card(cell) {
        Some(card) => LineageRec::from(&card, handle.tank.species_parent(card.species)),
        None => LineageRec::empty(),
    }
}

/// Copy a UTF-8 string into `buf` (NUL-terminated when it fits) and
/// return the byte length without the NUL. `cap == 0` just measures.
fn write_str(s: &str, buf: *mut u8, cap: u32) -> u32 {
    let bytes = s.as_bytes();
    if !buf.is_null() && cap as usize > bytes.len() {
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
            *buf.add(bytes.len()) = 0;
        }
    }
    bytes.len() as u32
}

#[no_mangle]
pub extern "C" fn tank_species_name(handle: *const TankHandle, species: u16, buf: *mut u8, cap: u32) -> u32 {
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return 0;
    };
    write_str(handle.tank.species_name(species), buf, cap)
}

#[no_mangle]
pub extern "C" fn tank_species_count(handle: *const TankHandle) -> u32 {
    unsafe { handle.as_ref() }.map(|h| h.tank.species_count() as u32).unwrap_or(0)
}

/// Fill `out` with up to `cap` saved species and their ancestors (id
/// order, parents first). Unsaved life is omitted. Returns the total
/// count so a second call can size the buffer.
#[no_mangle]
pub extern "C" fn tank_species_tree(handle: *const TankHandle, out: *mut SpeciesNodeRec, cap: u32) -> u32 {
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return 0;
    };
    let tree = handle.tank.species_tree();
    if !out.is_null() {
        for (i, n) in tree.iter().take(cap as usize).enumerate() {
            unsafe { *out.add(i) = SpeciesNodeRec::from(n) };
        }
    }
    tree.len() as u32
}

#[no_mangle]
pub extern "C" fn tank_mark_species_archived(handle: *mut TankHandle, species: u16, on: u8) -> u8 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return 0;
    };
    u8::from(handle.tank.mark_species_archived(species, on != 0))
}

/// Archive the organism of a living cell: writes the specimen bytes into
/// `buf` (two-call sizing like `tank_save`) and pins its species.
#[no_mangle]
pub extern "C" fn tank_specimen(handle: *mut TankHandle, cell: u32, buf: *mut u8, cap: usize) -> usize {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return 0;
    };
    let Some(sp) = handle.tank.archive_specimen(cell) else {
        return 0;
    };
    let bytes = sp.to_bytes();
    if buf.is_null() || cap < bytes.len() {
        return bytes.len();
    }
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len()) };
    bytes.len()
}

fn specimen_from(bytes: *const u8, len: usize) -> Option<Specimen> {
    if bytes.is_null() || len == 0 {
        return None;
    }
    Specimen::from_bytes(unsafe { std::slice::from_raw_parts(bytes, len) })
}

#[no_mangle]
pub extern "C" fn tank_specimen_info(bytes: *const u8, len: usize) -> SpecimenInfoRec {
    match specimen_from(bytes, len) {
        Some(sp) => SpecimenInfoRec::from(&sp),
        None => SpecimenInfoRec::empty(),
    }
}

/// `which`: 0 display name · 1 generated species name · 2 lineage
/// ("A → B → C") · 3 notes · 4 the whole archive page as text.
#[no_mangle]
pub extern "C" fn tank_specimen_text(bytes: *const u8, len: usize, which: u8, buf: *mut u8, cap: u32) -> u32 {
    let Some(sp) = specimen_from(bytes, len) else {
        return 0;
    };
    let s = match which {
        0 => sp.display_name.clone(),
        1 => sp.species_name.clone(),
        2 => sp.lineage.join(" → "),
        3 => sp.notes.clone(),
        _ => sp.page_text(),
    };
    write_str(&s, buf, cap)
}

/// Copy the pixel-exact RGBA portrait (w·h·4 bytes). Returns bytes needed.
#[no_mangle]
pub extern "C" fn tank_specimen_portrait(bytes: *const u8, len: usize, out: *mut u8, cap: usize) -> usize {
    let Some(sp) = specimen_from(bytes, len) else {
        return 0;
    };
    if !out.is_null() && cap >= sp.portrait.len() {
        unsafe { ptr::copy_nonoverlapping(sp.portrait.as_ptr(), out, sp.portrait.len()) };
    }
    sp.portrait.len()
}

/// Write a copy of the specimen with a new display name and/or notes
/// (null pointer = keep). Two-call sizing.
#[no_mangle]
pub extern "C" fn tank_specimen_edit(
    bytes: *const u8,
    len: usize,
    name: *const u8,
    name_len: usize,
    notes: *const u8,
    notes_len: usize,
    out: *mut u8,
    cap: usize,
) -> usize {
    let Some(mut sp) = specimen_from(bytes, len) else {
        return 0;
    };
    let text = |p: *const u8, n: usize| -> Option<String> {
        if p.is_null() {
            return None;
        }
        let s = unsafe { std::slice::from_raw_parts(p, n) };
        Some(String::from_utf8_lossy(s).into_owned())
    };
    if let Some(n) = text(name, name_len) {
        let n = n.trim();
        sp.display_name = if n.is_empty() { sp.species_name.clone() } else { n.to_string() };
    }
    if let Some(n) = text(notes, notes_len) {
        sp.notes = n;
    }
    let bytes = sp.to_bytes();
    if !out.is_null() && cap >= bytes.len() {
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    }
    bytes.len()
}

/// Place one copy of an archived creature at a pixel. UINT32_MAX = failed.
#[no_mangle]
pub extern "C" fn tank_seed_specimen(handle: *mut TankHandle, bytes: *const u8, len: usize, px: u32, py: u32) -> u32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return u32::MAX;
    };
    let Some(sp) = specimen_from(bytes, len) else {
        return u32::MAX;
    };
    handle.tank.seed_specimen(&sp, px as u16, py as u16).unwrap_or(u32::MAX)
}

/// New tank from the archive: `n` copies spread in open water. Returns placed.
#[no_mangle]
pub extern "C" fn tank_seed_specimen_copies(handle: *mut TankHandle, bytes: *const u8, len: usize, n: u32) -> u32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return 0;
    };
    let Some(sp) = specimen_from(bytes, len) else {
        return 0;
    };
    handle.tank.seed_specimen_copies(&sp, n)
}

#[no_mangle]
pub extern "C" fn tank_save(handle: *mut TankHandle, buf: *mut u8, cap: usize) -> usize {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return 0;
    };
    let Ok(bytes) = handle.tank.save_bytes() else {
        return 0;
    };
    if buf.is_null() || cap < bytes.len() {
        return bytes.len();
    }
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len()) };
    bytes.len()
}

#[no_mangle]
pub extern "C" fn tank_load(bytes: *const u8, len: usize) -> *mut TankHandle {
    if bytes.is_null() || len == 0 {
        return ptr::null_mut();
    }
    let slice = unsafe { std::slice::from_raw_parts(bytes, len) };
    match Tank::load_bytes(slice) {
        Ok(tank) => Box::into_raw(Box::new(TankHandle {
            tank,
            rgba: Vec::new(),
        })),
        Err(_) => ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn tank_tick_count(handle: *const TankHandle) -> u64 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.tick_count())
        .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn tank_day_factor(handle: *const TankHandle) -> f32 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.day_factor_now())
        .unwrap_or(0.0)
}

#[no_mangle]
pub extern "C" fn tank_is_day(handle: *const TankHandle) -> u8 {
    unsafe { handle.as_ref() }
        .map(|h| u8::from(h.tank.is_day_now()))
        .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn tank_seed(handle: *const TankHandle) -> u64 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.seed())
        .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn tank_t_base(handle: *const TankHandle) -> f32 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.t_base())
        .unwrap_or(0.0)
}

/// Whole-tank temperature °C (`T_tank = T_base + 0.4·T_direct + 0.6·S`).
#[no_mangle]
pub extern "C" fn tank_temperature(handle: *const TankHandle) -> f32 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.t_tank())
        .unwrap_or(0.0)
}

/// Air-side humidity 0..=255 at a visible physics cell (water reads 255).
#[no_mangle]
pub extern "C" fn tank_humidity_at(handle: *const TankHandle, vx: i32, vy: i32) -> u8 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.humidity_at(vx, vy))
        .unwrap_or(0)
}

/// Local THERMO excess °C at a visible physics cell.
#[no_mangle]
pub extern "C" fn tank_t_local_at(handle: *const TankHandle, vx: i32, vy: i32) -> f32 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.t_local_at(vx, vy))
        .unwrap_or(0.0)
}

/// Mean cloud factor 0..=1 (`1` = clear).
#[no_mangle]
pub extern "C" fn tank_cloud_mean(handle: *const TankHandle) -> f32 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.cloud_mean())
        .unwrap_or(1.0)
}

#[no_mangle]
pub extern "C" fn tank_light_slider(handle: *const TankHandle) -> f32 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.light_slider())
        .unwrap_or(0.0)
}

/// Built-in Phase 4 starter specimen (`kind`: 0 moss, 1 plant, 2 crown,
/// 3 rooting, 4 heater). Two-call sizing like `tank_specimen`.
#[no_mangle]
pub extern "C" fn tank_starter_specimen(kind: u8, buf: *mut u8, cap: usize) -> usize {
    let Some(sp) = Specimen::starter(kind) else {
        return 0;
    };
    let bytes = sp.to_bytes();
    if buf.is_null() || cap < bytes.len() {
        return bytes.len();
    }
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len()) };
    bytes.len()
}

/// Take the scoop bucket filled by enclosure release (tool 16 / op 3).
/// Null when nothing is pending.
#[no_mangle]
pub extern "C" fn tank_take_enclosure_bucket(handle: *mut TankHandle) -> *mut Carried {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ptr::null_mut();
    };
    match handle.tank.take_enclosure_bucket() {
        Some(inner) => Box::into_raw(Box::new(Carried { inner })),
        None => ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn tank_set_consent(handle: *mut TankHandle, on: u8) {
    if let Some(handle) = unsafe { handle.as_mut() } {
        handle.tank.telemetry_mut().set_consent(on != 0);
    }
}

#[no_mangle]
pub extern "C" fn tank_telemetry_kill(handle: *mut TankHandle, killed: u8) {
    if let Some(handle) = unsafe { handle.as_mut() } {
        handle.tank.telemetry_mut().set_killed(killed != 0);
    }
}

#[no_mangle]
pub extern "C" fn tank_telemetry_push(handle: *mut TankHandle, kind: u32, arg0: u32) -> u8 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return 0;
    };
    u8::from(handle.tank.telemetry_mut().push(kind, arg0))
}

#[no_mangle]
pub extern "C" fn tank_telemetry_drain(handle: *mut TankHandle, out: *mut TelRec, cap: u32) -> u32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return 0;
    };
    if out.is_null() || cap == 0 {
        return handle.tank.telemetry_mut().len() as u32;
    }
    let mut events: Vec<TelEvent> = Vec::new();
    handle
        .tank
        .telemetry_mut()
        .drain_max(cap as usize, &mut events);
    for (i, ev) in events.iter().enumerate() {
        unsafe {
            *out.add(i) = TelRec {
                kind: ev.kind,
                arg0: ev.arg0,
            };
        }
    }
    events.len() as u32
}

#[no_mangle]
pub extern "C" fn tank_glyph(codepoint: u32) -> u64 {
    glyph_bits(codepoint)
}

impl InspectRec {
    fn empty() -> Self {
        Self {
            hit: 0,
            material: 0,
            sat: 0,
            moisture: 0,
            variant: 0,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
            owner: 0,
            vx: 0,
            vy: 0,
        }
    }

    fn from(src: Inspect) -> Self {
        Self {
            hit: u8::from(src.hit),
            material: src.material,
            sat: src.sat,
            moisture: src.moisture,
            variant: src.variant,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
            owner: src.owner,
            vx: src.vx,
            vy: src.vy,
        }
    }
}

impl CellInspectRec {
    fn empty() -> Self {
        Self {
            cell: u32::MAX,
            org: 0,
            genotype: 0,
            species: u16::MAX,
            mode: 0,
            energy_frac: 0,
            generation: 0,
            org_cells: 0,
            glued_kin: 0,
            age_ticks: 0,
            max_age_ticks: 0,
            children: 0,
            stress: 0,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
            bbox_x0: 0,
            bbox_y0: 0,
            bbox_x1: 0,
            bbox_y1: 0,
        }
    }
}

impl LineageRec {
    fn empty() -> Self {
        Self {
            cell: u32::MAX,
            org: 0,
            genotype: 0,
            species: u16::MAX,
            parent_species: u16::MAX,
            generation: 0,
            mutations_since_seed: 0,
            species_depth: 0,
            kin_alive: 0,
            descendants_alive: 0,
            archived_relatives: 0,
            children: 0,
        }
    }

    fn from(c: &LineageCard, parent: Option<u16>) -> Self {
        Self {
            cell: c.cell,
            org: c.org,
            genotype: c.genotype,
            species: c.species,
            parent_species: parent.unwrap_or(u16::MAX),
            generation: c.generation,
            mutations_since_seed: c.mutations_since_seed,
            species_depth: c.species_depth,
            kin_alive: c.kin_alive,
            descendants_alive: c.descendants_alive,
            archived_relatives: c.archived_relatives,
            children: c.children,
        }
    }
}

impl SpeciesNodeRec {
    fn from(n: &SpeciesNode) -> Self {
        Self {
            id: n.id,
            parent: n.parent,
            depth: n.depth,
            archived: u8::from(n.archived),
            extinct: u8::from(n.extinct),
            r: n.colour[0],
            g: n.colour[1],
            b: n.colour[2],
            _pad: 0,
            live_cells: n.live_cells,
            live_genotypes: n.live_genotypes,
            forms: n.forms,
            born: n.born,
            first_tick: n.first_tick,
        }
    }
}

impl SpecimenInfoRec {
    fn empty() -> Self {
        Self {
            id: 0,
            tank_seed: 0,
            tick: 0,
            archive_parent: 0,
            has_archive_parent: 0,
            r: 0,
            g: 0,
            b: 0,
            species_depth: 0,
            mutations_since_seed: 0,
            generation: 0,
            cells: 0,
            slots: 0,
            age_ticks: 0,
            max_age_ticks: 0,
            children: 0,
            kin_alive: 0,
            descendants_alive: 0,
            portrait_w: 0,
            portrait_h: 0,
        }
    }

    fn from(sp: &Specimen) -> Self {
        Self {
            id: sp.id,
            tank_seed: sp.tank_seed,
            tick: sp.tick,
            archive_parent: sp.archive_parent.unwrap_or(0),
            has_archive_parent: u8::from(sp.archive_parent.is_some()),
            r: sp.colour[0],
            g: sp.colour[1],
            b: sp.colour[2],
            species_depth: sp.species_depth,
            mutations_since_seed: sp.mutations_since_seed,
            generation: sp.generation,
            cells: sp.stats.cells,
            slots: sp.stats.slots,
            age_ticks: sp.stats.age_ticks,
            max_age_ticks: sp.stats.max_age_ticks,
            children: sp.stats.children,
            kin_alive: sp.stats.kin_alive,
            descendants_alive: sp.stats.descendants_alive,
            portrait_w: sp.portrait_w,
            portrait_h: sp.portrait_h,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_paused() -> TankInputC {
        TankInputC {
            grav_x: 0.0,
            grav_y: -1.0,
            paused: 1,
            always_day: 0,
            swirl: 0,
            _pad1: 0,
            light_slider: 1.0,
            t_base: 15.0,
            flash_x: 0,
            flash_y: 0,
            flash_r: 0,
            tools: ptr::null(),
            tool_count: 0,
        }
    }

    #[test]
    fn abi_sizes_match_the_header() {
        assert_eq!(std::mem::size_of::<ToolEventC>(), 10);
        // grav(8) + flags(4) + slider/t_base(8) + flash(12) + tools*(8) + count(4) + pad(4) = 48
        assert_eq!(std::mem::size_of::<TankInputC>(), 48);
        assert_eq!(std::mem::offset_of!(TankInputC, tools), 32);
        assert_eq!(std::mem::offset_of!(TankInputC, flash_x), 20);
        assert_eq!(std::mem::size_of::<InspectRec>(), 20);
        assert_eq!(std::mem::size_of::<EventRec>(), 8);
        assert_eq!(std::mem::size_of::<TelRec>(), 8);
        assert_eq!(std::mem::size_of::<CellInspectRec>(), 52);
        assert_eq!(std::mem::offset_of!(CellInspectRec, bbox_x0), 44);
        assert_eq!(std::mem::size_of::<LineageRec>(), 40);
        assert_eq!(std::mem::size_of::<SpeciesNodeRec>(), 40);
        assert_eq!(std::mem::offset_of!(SpeciesNodeRec, first_tick), 32);
        assert_eq!(std::mem::size_of::<SpecimenInfoRec>(), 80);
        assert_eq!(std::mem::offset_of!(SpecimenInfoRec, generation), 40);
    }

    #[test]
    fn scoop_release_and_specimen_go_through_the_abi() {
        let handle = tank_preset(3);
        let input = TankInputC {
            paused: 0,
            always_day: 1,
            ..input_paused()
        };
        for _ in 0..200 {
            tank_tick(handle, &input);
        }
        let before = tank_live_cells(handle);
        assert!(before > 1);
        // Find a living cell by scanning the top rows.
        let mut w = 0u32;
        let mut h = 0u32;
        let _ = tank_pixels(handle, &mut w, &mut h);
        let mut found = CellInspectRec::empty();
        'scan: for py in 0..h {
            for px in 0..w {
                let rec = tank_inspect_cell(handle, px, py);
                if rec.cell != u32::MAX {
                    found = rec;
                    break 'scan;
                }
            }
        }
        assert_ne!(found.cell, u32::MAX, "a living cell is on screen");
        assert!(found.bbox_x1 >= found.bbox_x0 && found.bbox_y1 >= found.bbox_y0);
        let lin = tank_lineage(handle, found.cell);
        assert_eq!(lin.cell, found.cell);
        assert_ne!(lin.species, u16::MAX);
        let mut name = [0u8; 64];
        let n = tank_species_name(handle, lin.species, name.as_mut_ptr(), 64);
        assert!(n > 0 && name[n as usize] == 0, "NUL-terminated name");
        assert!(std::str::from_utf8(&name[..n as usize]).unwrap().contains(' '));
        let living = tank_species_tree(handle, ptr::null_mut(), 0);
        assert!(living >= 1, "living species are on the tree before anyone saves them");

        // Archive it, inspect the record, seed a new tank from it.
        let need = tank_specimen(handle, found.cell, ptr::null_mut(), 0);
        assert!(need > 0);
        let mut buf = vec![0u8; need];
        assert_eq!(tank_specimen(handle, found.cell, buf.as_mut_ptr(), need), need);
        let count = tank_species_tree(handle, ptr::null_mut(), 0);
        assert!(count >= 1, "saving a creature puts its species on the tree");
        let mut nodes = vec![
            SpeciesNodeRec {
                id: 0,
                parent: 0,
                depth: 0,
                archived: 0,
                extinct: 0,
                r: 0,
                g: 0,
                b: 0,
                _pad: 0,
                live_cells: 0,
                live_genotypes: 0,
                forms: 0,
                born: 0,
                first_tick: 0,
            };
            count as usize
        ];
        assert_eq!(tank_species_tree(handle, nodes.as_mut_ptr(), count), count);
        assert!(nodes.iter().any(|n| n.id == lin.species && n.live_cells > 0 && n.archived != 0));
        let info = tank_specimen_info(buf.as_ptr(), need);
        assert!(info.cells >= 1 && info.portrait_w >= 1);
        let mut text = vec![0u8; 512];
        let tn = tank_specimen_text(buf.as_ptr(), need, 4, text.as_mut_ptr(), 512);
        assert!(tn > 0);
        let pn = tank_specimen_portrait(buf.as_ptr(), need, ptr::null_mut(), 0);
        assert_eq!(pn, info.portrait_w as usize * info.portrait_h as usize * 4);
        let note = b"swims to the light";
        let en = tank_specimen_edit(buf.as_ptr(), need, ptr::null(), 0, note.as_ptr(), note.len(), ptr::null_mut(), 0);
        let mut edited = vec![0u8; en];
        assert_eq!(
            tank_specimen_edit(buf.as_ptr(), need, ptr::null(), 0, note.as_ptr(), note.len(), edited.as_mut_ptr(), en),
            en
        );
        let nn = tank_specimen_text(edited.as_ptr(), en, 3, text.as_mut_ptr(), 512);
        assert_eq!(&text[..nn as usize], note);
        let fresh = tank_preset(9);
        let had = tank_live_cells(fresh);
        assert_eq!(tank_seed_specimen_copies(fresh, edited.as_ptr(), en, 3), 3);
        assert_eq!(tank_live_cells(fresh), had + 3);
        tank_free(fresh);

        // Scoop the cell's neighbourhood and put it back elsewhere.
        let cx = found.bbox_x0 as i32;
        let cy = found.bbox_y0 as i32;
        let carried = tank_scoop(handle, cx, cy, 6);
        assert!(!carried.is_null());
        let lifted = tank_carried_cells(carried);
        assert!(lifted >= 1);
        assert_eq!(tank_live_cells(handle) + lifted, before);
        // Out of bounds: refused, bucket still valid.
        assert_eq!(tank_release(handle, carried, -5, -5), 0);
        assert_eq!(tank_carried_cells(carried), lifted);
        // Somewhere in open water near the top of the preset: try a few spots.
        let mut ok = false;
        for dx in [40, -40, 80, -80, 120, -120] {
            if tank_release(handle, carried, cx + dx, cy) == 1 {
                ok = true;
                break;
            }
        }
        assert!(ok, "the bucket found room");
        assert_eq!(tank_live_cells(handle), before);
        // Remove tool via the ABI (physics coords): anything there goes.
        let _ = tank_remove_at(handle, 1, 1, 1);
        tank_free(handle);
        tank_scoop(ptr::null_mut(), 0, 0, 1);
        tank_discard(ptr::null_mut(), ptr::null_mut());
    }

    #[test]
    fn save_roundtrip_keeps_the_pixel_hash() {
        let handle = tank_new(4, 16, 20);
        assert!(!handle.is_null());
        let input = TankInputC {
            paused: 0,
            ..input_paused()
        };
        for _ in 0..15 {
            tank_tick(handle, &input);
        }
        let hash = tank_pixel_hash(handle);
        let mut w = 0u32;
        let mut h = 0u32;
        let pixels = tank_pixels(handle, &mut w, &mut h);
        assert_eq!((w, h), (48, 60));
        assert!(!pixels.is_null());
        let needed = tank_save(handle, ptr::null_mut(), 0);
        assert!(needed > 100);
        let mut buf = vec![0u8; needed];
        let wrote = tank_save(handle, buf.as_mut_ptr(), buf.len());
        assert_eq!(wrote, needed);
        let loaded = tank_load(buf.as_ptr(), wrote);
        assert!(!loaded.is_null());
        assert_eq!(tank_pixel_hash(loaded), hash);
        assert_eq!(tank_tick_count(loaded), tank_tick_count(handle));
        assert_eq!(tank_poll_event(handle).kind, 0);
        tank_free(handle);
        tank_free(loaded);
        tank_free(ptr::null_mut());
    }

    #[test]
    fn bad_size_and_bad_save_return_null() {
        assert!(tank_new(1, 1, 1).is_null());
        assert!(tank_load(ptr::null(), 0).is_null());
        let junk = [1u8, 2, 3, 4];
        assert!(tank_load(junk.as_ptr(), junk.len()).is_null());
    }

    #[test]
    fn telemetry_is_off_until_consent() {
        let handle = tank_new(1, 8, 8);
        assert_eq!(tank_telemetry_push(handle, 3, 1), 0);
        let mut recs = [TelRec { kind: 0, arg0: 0 }; 4];
        assert_eq!(tank_telemetry_drain(handle, recs.as_mut_ptr(), 4), 0);
        tank_set_consent(handle, 1);
        assert_eq!(tank_telemetry_push(handle, 3, 9), 1);
        assert_eq!(tank_telemetry_drain(handle, recs.as_mut_ptr(), 4), 1);
        assert_eq!(recs[0].kind, 3);
        assert_eq!(recs[0].arg0, 9);
        tank_telemetry_kill(handle, 1);
        assert_eq!(tank_telemetry_push(handle, 1, 1), 0);
        tank_free(handle);
    }

    #[test]
    fn header_lists_the_entry_points() {
        let header = include_str!("../include/tank.h");
        for name in [
            "tank_new",
            "tank_empty",
            "tank_seed_algae",
            "tank_live_cells",
            "tank_swipe_film",
            "tank_tick",
            "tank_pixels",
            "tank_inspect",
            "tank_poll_event",
            "tank_scoop",
            "tank_release",
            "tank_save",
            "tank_load",
            "tank_free",
            "tank_undo",
            "tank_glyph",
            "tank_set_consent",
            "tank_inspect_cell",
            "tank_lineage",
            "tank_species_name",
            "tank_species_tree",
            "tank_discard",
            "tank_remove_at",
            "tank_specimen",
            "tank_specimen_info",
            "tank_specimen_text",
            "tank_specimen_portrait",
            "tank_specimen_edit",
            "tank_seed_specimen",
            "tank_seed_specimen_copies",
            "tank_mark_species_archived",
            "tank_org_bbox",
            "tank_carried_cells",
            "tank_temperature",
            "tank_humidity_at",
            "tank_t_local_at",
            "tank_cloud_mean",
            "tank_starter_specimen",
            "tank_take_enclosure_bucket",
        ] {
            assert!(header.contains(name), "{name} missing from tank.h");
        }
    }

    #[test]
    fn phase4_abi_exposes_climate_and_starters() {
        let handle = tank_preset(2);
        assert!(!handle.is_null());
        let t = tank_temperature(handle);
        assert!(t > 0.0 && t < 40.0, "t={t}");
        let _ = tank_humidity_at(handle, 10, 10);
        let _ = tank_t_local_at(handle, 10, 10);
        assert!(tank_cloud_mean(handle) > 0.5);
        let need = tank_starter_specimen(0, ptr::null_mut(), 0);
        assert!(need > 0);
        let mut buf = vec![0u8; need];
        assert_eq!(tank_starter_specimen(0, buf.as_mut_ptr(), need), need);
        let info = tank_specimen_info(buf.as_ptr(), need);
        assert!(info.cells >= 1 && info.portrait_w >= 1);
        assert_eq!(tank_starter_specimen(99, ptr::null_mut(), 0), 0);
        assert!(tank_take_enclosure_bucket(handle).is_null());
        tank_free(handle);
    }

    #[test]
    fn nordic_glyph_comes_through_the_abi() {
        assert_ne!(tank_glyph('æ' as u32), 0);
        assert_ne!(tank_glyph('ø' as u32), 0);
        assert_ne!(tank_glyph('å' as u32), 0);
    }

    #[test]
    fn empty_tank_and_undo_are_exposed() {
        let handle = tank_empty(3);
        assert!(!handle.is_null());
        let tool = ToolEventC {
            kind: 1,
            material: 2,
            x: 4,
            y: 4,
            r: 2,
            variant: 2,
            _pad: 0,
        };
        let input = TankInputC {
            tools: &tool,
            tool_count: 1,
            ..input_paused()
        };
        tank_tick(handle, &input);
        assert_eq!(tank_undo_depth(handle), 1);
        assert_eq!(tank_undo(handle), 1);
        assert_eq!(tank_undo_depth(handle), 0);
        tank_free(handle);
    }
}

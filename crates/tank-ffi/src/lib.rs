#![allow(clippy::not_unsafe_ptr_arg_deref)]
//! C ABI described in `include/tank.h`.
//!
//! The pointer returned by [`tank_pixels`] is valid until the next
//! `tank_pixels`, `tank_free`, or `tank_load` on that handle. The core
//! is single-threaded; the shell calls it from one queue.

use std::ptr;

use tank_core::{glyph_bits, Inspect, Tank, TankInput, TelEvent, ToolEvent};

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
    pub _pad: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TankInputC {
    pub grav_x: f32,
    pub grav_y: f32,
    pub paused: u8,
    pub always_day: u8,
    pub _pad0: u8,
    pub _pad1: u8,
    pub light_slider: f32,
    pub t_base: f32,
    pub tools: *const ToolEventC,
    pub tool_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InspectRec {
    pub hit: u8,
    pub material: u8,
    pub sat: u8,
    pub _pad: u8,
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

/// Opaque scoop payload. Phase 3 fills it; Phase 0 always returns null.
pub struct Carried {
    _private: u8,
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
        }));
    }
    handle.tank.tick(&TankInput {
        grav_x: input.grav_x,
        grav_y: input.grav_y,
        paused: input.paused != 0,
        always_day: input.always_day != 0,
        light_slider: input.light_slider,
        t_base: input.t_base,
        tools,
    });
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

#[no_mangle]
pub extern "C" fn tank_scoop(
    _handle: *mut TankHandle,
    _cx: i32,
    _cy: i32,
    _r: i32,
) -> *mut Carried {
    ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn tank_release(
    _handle: *mut TankHandle,
    _carried: *mut Carried,
    _x: i32,
    _y: i32,
) -> u8 {
    0
}

#[no_mangle]
pub extern "C" fn tank_carried_free(carried: *mut Carried) {
    if carried.is_null() {
        return;
    }
    unsafe { drop(Box::from_raw(carried)) };
}

/// Bytes needed when `buf` is null or `cap` is short. `0` means the save
/// failed. Otherwise the number of bytes written.
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

#[no_mangle]
pub extern "C" fn tank_light_slider(handle: *const TankHandle) -> f32 {
    unsafe { handle.as_ref() }
        .map(|h| h.tank.light_slider())
        .unwrap_or(0.0)
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
            _pad: 0,
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
            _pad: 0,
            owner: src.owner,
            vx: src.vx,
            vy: src.vy,
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
            _pad0: 0,
            _pad1: 0,
            light_slider: 1.0,
            t_base: 15.0,
            tools: ptr::null(),
            tool_count: 0,
        }
    }

    #[test]
    fn abi_sizes_match_the_header() {
        assert_eq!(std::mem::size_of::<ToolEventC>(), 10);
        assert_eq!(std::mem::size_of::<TankInputC>(), 40);
        assert_eq!(std::mem::align_of::<TankInputC>(), 8);
        assert_eq!(std::mem::offset_of!(TankInputC, tools), 24);
        assert_eq!(std::mem::size_of::<InspectRec>(), 16);
        assert_eq!(std::mem::size_of::<EventRec>(), 8);
        assert_eq!(std::mem::size_of::<TelRec>(), 8);
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
            "tank_tick",
            "tank_pixels",
            "tank_inspect",
            "tank_poll_event",
            "tank_scoop",
            "tank_release",
            "tank_save",
            "tank_load",
            "tank_free",
            "tank_glyph",
            "tank_set_consent",
        ] {
            assert!(header.contains(name), "{name} missing from tank.h");
        }
    }

    #[test]
    fn nordic_glyph_comes_through_the_abi() {
        assert_ne!(tank_glyph('æ' as u32), 0);
        assert_ne!(tank_glyph('ø' as u32), 0);
        assert_ne!(tank_glyph('å' as u32), 0);
    }
}

# Phase 8 — Host / client contract *(sketch)*

**Status:** live sketch + spike code (2026-10-12). Stack: ship =
**egui + wgpu / winit**; soak/dev = **macroquad**. Owner confirm: client
is a **viewer** (camera + limited commands); host owns sim.
[`PHASE8_SCOPE.md`](PHASE8_SCOPE.md), [`PHASE8_CLIENT_STACKS.md`](PHASE8_CLIENT_STACKS.md).

## Hard rules

1. **Host owns truth.** `step_world`, CA/mass, park/audit, organisms,
   climate clock, `SimSnapshot` save/load live only on the host.
2. **Client never mutates sat/mass/cells.** Paint, spawn, climate dials,
   pause — all go through **commands**; host applies or rejects.
3. **Presentation may lag.** Client paints the latest **view frame**; it
   must not call into `tick` / `step_world` itself.
4. **Single-player first.** In-process host+client remains the default.
   Out-of-process localhost IPC is landed; multiplayer / auth is later.

## Topology (spike → ship)

```text
Default (in-process):
┌──────────────────────────────────────────────────────────┐
│  Process                                                 │
│   ┌─────────────────┐  ViewFrame              ┌────────┐ │
│   │ Host (lib)      │ ──────────────────────► │ Client │ │
│   │ step / save     │ ◄────────────────────── │ A: egui│ │
│   └─────────────────┘  ClientCommand queue    │ wgpu   │ │
└──────────────────────────────────────────────────────────┘

Optional (OOP spike):
  wk-voxel-host  ←── TCP postcard (Hello/Cmd/Pull ↔ HelloAck/Frame) ──→  wk-voxel-client --connect
  Default bind 127.0.0.1:17420. One client. No auth.
```

Dev path unchanged: `wk-voxel-app` (macroquad) may keep calling
`step_world` in-process until retired. Multiplayer not in scope.

## Two snapshot kinds (do not conflate)

| Kind | Purpose | Who uses it |
|------|---------|-------------|
| **`SimSnapshot`** (`save.rs`, schema v19+) | Full durable world | Host save/load only; **not** the per-frame client feed |
| **`ViewFrame`** (new, sketch below) | What to draw this frame | Host → client each present tick |

Shipping the whole `World` every frame is a non-goal. The view frame is
a **camera-window extract** plus HUD scalars.

## ViewFrame (host → client)

Versioned, spike-minimal. Fields are logical; encoding (postcard / custom
GPU upload) is an implementation choice in the spike.

```text
ViewFrame v1
├── header
│   ├── contract_version: u32        // VIEW_SCHEMA = 1
│   ├── sim_tick: u64
│   ├── paused: bool
│   ├── wrap_width: Option<i32>
│   ├── world_width_cols: i32
│   ├── world_height: i32
│   └── stream_window: Option<(focus_col, view_cols)>
├── sky / climate (presentation inputs — host-authored)
│   ├── day_night / sun_local / season_fraction / moon_phase
│   ├── wind_vx (effective)
│   └── glitter / overlay enable flags mirrored from host settings
├── terrain strip (camera window only)
│   ├── origin_col: i32              // inclusive left of window
│   ├── cols: u32
│   ├── rows: u32                    // usually full height
│   └── cells: packed MaterialId + sat/pore bits  // draw-relevant only
├── overlays (window-culled samples)
│   ├── organism_draw_cells[]        // already resolved poses
│   ├── haze / humidity samples[]
│   ├── steam / pipe samples[]
│   └── optional: temp / mycelium / budget HUD scalars
└── host_info
    ├── last_step_ms / sim_fps estimate
    ├── park / TRACKED audit crumbs (HUD)
    └── command_acks[]               // ids rejected/applied
```

**Rules for terrain strip**

- Host fills from resident chunks after `sync_residency` for the
  client-requested focus (see commands).
- Client must not re-derive mass; it may shade (day/night, glitter) from
  sky fields + cell bits.
- Outside the strip: empty / undrawn — no full-map dump.

**Spike subset:** header + `sim_tick` + fake clear-color only is enough
for the empty-window spike. Terrain strip lands when presentation ports.

## ClientCommand (client → host)

Queue drained by host **before** or **between** `step_world` calls
(never mid-step). Commands are idempotent where practical; host ACKs.

| Command | Effect on host | Notes |
|---------|----------------|-------|
| `SetCamera { focus_col, view_cols, focus_y }` | Updates `stream_window` + Compact vertical band | Client owns pan UX; host owns residency |
| `SetPaused(bool)` / `StepOnce` | Gate `step_world` | Space / pause parity with demo |
| `SetSimSettings { … }` | Climate, steam, organisms_on, stream_disk, … | Sim-affecting dials only |
| `SetPresentHints { … }` | Optional: glitter on, overlay masks | If host stamps these into ViewFrame; else client-local |
| `Save { slot }` / `Load { slot }` | `SimSnapshot` I/O | Client never writes `.gvsesim` itself |
| `PaintTerrain { … }` / `SpawnOrganism { … }` | Editor mutations | Host validates; rejected → ACK error |
| `ResetWorld { seed? }` | Rebuild via worldgen | R key parity |
| `Quit` | Host teardown | Client closes window after ACK |

**Client-local (not commands):** window size, egui layout, keybind
display, zoom-if-any that does not change `view_cols` residency.

## Frame loop (in-process)

```text
1. client polls input → push ClientCommand[]
2. host drain commands → apply
3. if !paused: sync_residency(stream_window); step_world(...)
4. host build ViewFrame from window + HUD
5. client draw ViewFrame (wgpu) + egui HUD
```

Soak/headless path skips 1/5; Compact park=0 gate stays on host binary
with no client.

## Settings split

| Dial class | Owner | Examples |
|------------|-------|----------|
| Sim physics / climate | Host (`SetSimSettings`) | season, steam enable, organisms, evap, stream disk |
| Present-only | Client local (or hints) | HUD font, which overlay toggles are visible, vsync |
| Dual | Host stamps into ViewFrame; client may also toggle locally for debug | glitter, humidity overlay |

Demo `settings.rs` today mixes both in one `macroquad::ui` tree — ship
client should **split panels** so sim dials always go through commands.

## Versioning

| Constant | Bump when |
|----------|-----------|
| `VIEW_SCHEMA` | ViewFrame layout (v5 `sim`; v6 `host`; v7–8 `present`; v9 `focus_y`) |
| `COMMAND_SCHEMA` | Commands (v2 save; v3 SetSimSettings; v4 SetPresentHints; v5 `SetCamera.focus_y`) |
| `SIM_SCHEMA_VERSION` | unchanged role — durable save only (`save.rs`) |

Spike may hard-code v1 and reject mismatches with a clear HUD string.

## Acceptance for this sketch

- [x] Named message kinds: `ViewFrame`, `ClientCommand`, distinct from
      `SimSnapshot`.
- [x] Hard rules: no client mass mutation; single-player in-process first.
- [x] Command table covers camera, pause, save/load, settings, editors.
- [x] Spike implements empty window + tick counter via this loop
      (`crates/wk-voxel-client`).
- [x] Terrain strip in `ViewFrame` (schema v2) + egui blit; small stamped
      world + `tick` on host.
- [x] Sky present fields (schema v3) — day/night/phase/moon/season; air
      cells filled with height-phased sky; sun/moon marker in client.
- [x] Fuller palette (wetness / perm / mycelium) + water glitter on tops.
- [x] Host uses product [`step_world`] (climate fields); viewer throttled
      present (`VIEW_SCHEMA` 4 + `product_step`).
- [x] Compact host world: default stamp 4096×tropopause; `StreamWindow` +
      `sync_residency` under camera; strip band ~384 around sea level;
      `Host::for_tests()` keeps tiny ring for CI.
- [x] Out-of-process host: `wk-voxel-host` + postcard IPC; viewer
      `--connect HOST:PORT` (in-process still default).
- [x] Apple packaging scripts (Developer ID + notarize runbook);
      [`PHASE8_APPLE_PACK.md`](PHASE8_APPLE_PACK.md).
- [x] Host `Save` / `Load` / `ResetWorld` (`COMMAND_SCHEMA` 2); viewer
      F5 / F9 / R; works in-process and over IPC.
- [x] Viewer `--spawn-host` launches `wk-voxel-host` child (bind
      `127.0.0.1:0`, announce before stamp).
- [x] `SetSimSettings` + `ViewFrame.sim` (`COMMAND_SCHEMA` 3).
- [x] `HostInfo` crumbs on `ViewFrame`.
- [x] `SetPresentHints` + humidity / steam / caveH strip washes
      (`VIEW_SCHEMA` 8 / `COMMAND_SCHEMA` 4).
- [x] Broader Compact default view (720) + vertical pan `focus_y`
      (`VIEW_SCHEMA` 9 / `COMMAND_SCHEMA` 5); `[`/`]` view width.
- [ ] Owner Mac notarize — **pinned until 2026-11-15**.
- [ ] Later: Steam storefront / IPC harden / MAS.

## Non-goals

Netcode, delta compression, prediction, WASM, Godot/Bevy bridges,
replacing soak harness, sending full `SimSnapshot` every frame.

## Spike crate

`crates/wk-voxel-client` — `winit` 0.30.5 + `wgpu` 22 + `egui` 0.29
(pinned under rustc 1.83; transitive edition2024 crates forced down).
In-process host (`crates/wk-voxel-client/src/lib.rs`) emits `ViewFrame`
header only; egui HUD shows `sim_tick`. Keys: Space pause, N step,
←/→ pan focus, Esc quit.

Linux run deps (not needed for `cargo test`): `libxkbcommon-x11-0`,
Mesa (`mesa-vulkan-drivers` and/or GL). Spike enables PRIMARY|GL backends.
macOS uses Metal by default. Set `WK_VOXEL_FORCE_FALLBACK_ADAPTER=1` only
for software/GL fallback (Linux Xvfb spikes) — forcing fallback on Mac
returns no adapter and used to abort inside winit’s launch callback.

## Next

1. [x] Land this sketch + A spike empty window.
2. [x] Terrain strip (`VIEW_SCHEMA` 2) on A.
3. [x] Sky present on A.
4. [x] Fuller palette / water glitter on A.
5. [x] Host `step_world` + viewer framing (`VIEW_SCHEMA` 4).
6. [x] Compact-sized in-process host + stream residency.
7. [x] Out-of-process host binary + localhost IPC.
8. [x] Apple notarize scripts + owner Mac runbook.
9. [x] Host save/load/reset commands (`COMMAND_SCHEMA` 2).
10. [x] Viewer `--spawn-host` child process.
11. [x] SetSimSettings + ViewFrame.sim.
12. Present parity / HostInfo crumbs per [`PHASE8_BACKLOG.md`](PHASE8_BACKLOG.md).
13. Keep `wk-voxel-app` (macroquad) as soak/dev until A presentation parity.

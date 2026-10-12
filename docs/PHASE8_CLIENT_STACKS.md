# Phase 8 — Client stack comparison

**Status:** comparison only (2026-10-12). No production client coding
until the owner picks a stack (or parks Phase 8). See
[`PHASE8_SCOPE.md`](PHASE8_SCOPE.md) for ownership split and wedge plan.

## Baseline (what we have today)

| Fact | Detail |
|------|--------|
| Binary | `wk-voxel-app` — **one process**: sim step + draw + input |
| Render | **macroquad 0.4.14** immediate 2D (`draw_rectangle`, `Texture2D` atlas, sky/haze overlays in `atmosphere.rs` / `terrain_atlas.rs`) |
| UI | **`macroquad::ui`** (`settings.rs` windows/sliders) — **not** standalone egui |
| Hostless | Headless soak / Compact harness already separate from the demo window |
| Ship bar | Prefer a client that can pass **Apple** (notarized `.app` / MAS) and/or **Steam** inspection without rewriting mass rules |

Kernel constraint (unchanged): MS Paint 1× on the **sim** side; client
may grow richer presentation. Client **never** mutates sat/mass.

## Evaluation criteria

Weighted toward Phase 8 intent (shippable presentation, not research FX):

1. **Host/client hygiene** — easy to keep mass/CA out of the client crate
2. **Rewrite cost** — how much of today’s draw + settings must move
3. **Steam desktop** — packaging, deps, updater story
4. **Apple** — Metal path, notarization / MAS friction
5. **UI polish** — settings, HUD, quit/confirm, accessibility later
6. **Ops / CI** — Xvfb soaks, headless CI, agent workflow
7. **Lock-in / churn** — framework upgrades pulling the sim
8. **Spike size** — empty window + host tick pump before gameplay

Scores are relative (**Strong / OK / Weak**), not numeric absolutes.

---

## Stack A — egui + wgpu / winit

**Shape:** Rust client binary. `winit` window + `wgpu` surface for
atlas/sky/overlays; `egui` (via `egui-wgpu` / `egui-winit`) for settings
and HUD. Sim stays in a host crate or in-process host API behind a
snapshot/command contract.

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Host/client hygiene | **Strong** | No game ECS tempting mass into the world; thin present layer |
| Rewrite cost | **OK** | Full redraw of atlas/sky path off macroquad; settings leave `macroquad::ui` → egui widgets |
| Steam | **Strong** | Common indie Rust path; ship a native binary + Steamworks later |
| Apple | **Strong** | wgpu → Metal; notarized `.app` via `cargo-bundle` / custom scripts; MAS possible with care |
| UI polish | **OK→Strong** | egui is production-usable for tools/settings; not a full “game UI kit” |
| Ops / CI | **Strong** | Headless-ish tests without Godot; Xvfb still fine for pixels |
| Lock-in / churn | **Strong** | You own the render loop; upgrades are crate-level |
| Spike size | **Small–medium** | Clear window + clear color + egui “hello” + fake host tick |

**Pros**

- Aligns with “sim owns truth, client paints snapshots.”
- Single language/runtime for ship client + host.
- Corrects the Phase 8 draft slip: demo settings are **macroquad::ui**,
  not egui — adopting A means a deliberate UI port, not “already done.”
- Good Apple Metal story without bundling a full engine.

**Cons / risks**

- Every draw helper today assumes macroquad types (`Image`, `Texture2D`,
  `Color`). Port is mechanical but not free.
- No built-in scene graph / asset pipeline — fine for 2D atlas, painful
  if Phase 8+ wants 3D suddenly.
- Team must own packaging scripts (Steam depot, notarize).

**When A wins:** Steam and/or Apple desktop first; keep soak/macroquad
as a **dev viewer**; ship client is a second binary.

---

## Stack B — Bevy

**Shape:** Bevy app as the client. Host either (a) separate process
feeding snapshots, or (b) Bevy resource/plugin wrapping `wk-voxel` —
**(b) is the failure mode to avoid.**

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Host/client hygiene | **OK / Weak** | Ecosystem pushes “put sim in Bevy World”; needs hard crate boundary |
| Rewrite cost | **Weak→OK** | Different mental model (ECS schedules, assets); atlas as texture + custom pass |
| Steam | **Strong** | Desktop Bevy titles ship; packaging similar to other Rust natives |
| Apple | **OK→Strong** | wgpu/Metal; Bevy version pin matters for macOS CI |
| UI polish | **OK** | `bevy_egui` or Bevy UI; settings rewrite still required |
| Ops / CI | **OK** | Heavier compile; Xvfb works; version bumps can break CI |
| Lock-in / churn | **Weak** | Bevy majors move; render APIs shift |
| Spike size | **Medium** | Empty Bevy window + plugin that polls host is doable; easy to overbuild |

**Pros**

- Rendering / camera / input ecosystem if presentation grows (particles,
  post, richer FX) without inventing everything.
- One Rust toolchain still.

**Cons / risks**

- **Highest architectural smell risk:** organisms, park, or CA step
  “temporarily” living as Bevy systems. Phase 8 must forbid that in
  contract docs.
- Compile times and dependency surface dwarf A/D.
- Overkill if the ship client stays a 2D atlas + HUD for a long time.

**When B wins:** Owner expects a long-lived presentation engine (3D or
heavy FX) and accepts discipline + Bevy upgrade tax.

---

## Stack C — Godot 4 (GDExtension / Rust)

**Shape:** Godot project for window, UI, packaging; Rust GDExtension (or
separate host process + Godot as pure view) for glue. Sim kernel remains
Rust (`wk-voxel`).

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Host/client hygiene | **OK** | Natural process/editor split if host is external; FFI if in-proc |
| Rewrite cost | **Weak** | New scene tree, assets, input map; atlas blit via Godot APIs |
| Steam | **Strong** | Mature export presets / store lore |
| Apple | **OK** | Export templates + notarize; MAS/engine bundling quirks historically |
| UI polish | **Strong** | Editor + Control nodes beat egui for “game settings” feel |
| Ops / CI | **Weak** | Two toolchains; headless Godot + extension build in agents |
| Lock-in / churn | **OK** | Godot LTS helps; Rust binding versions lag |
| Spike size | **Medium–large** | Editor project + extension + host tick; more moving parts |

**Pros**

- Best **editor / storefront packaging** maturity of the four.
- Designers (if any) can tweak UI without touching Rust draw code.

**Cons / risks**

- Two runtimes and an FFI or IPC surface in every release.
- Agent/Xvfb workflow gets heavier (Godot binary + templates).
- Apple review can care about engine frameworks and entitlements more
  than a slim Rust binary.

**When C wins:** Owner prioritizes store tooling and UI craft over Rust
homogeneity, and accepts dual-runtime ops.

---

## Stack D — Keep macroquad + ship wrapper

**Shape:** Continue `wk-voxel-app` as the playable client. Add packaging
scripts (Steam depot, notarize `.app`), maybe a thin launcher. Optional:
split host API **inside** the same process first, still drawing with
macroquad.

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Host/client hygiene | **OK** | Can still split crates; easier to keep “one binary forever” |
| Rewrite cost | **Strong** | Smallest delta — wrap what exists |
| Steam | **OK** | Native Linux/Windows/macOS binary ships; polish is on you |
| Apple | **Weak→OK** | Notarized outside-MAS is feasible; MAS / inspection polish weaker |
| UI polish | **Weak** | `macroquad::ui` is fine for dials, weak for storefront UX |
| Ops / CI | **Strong** | Already proven (Xvfb, soaks, agents) |
| Lock-in / churn | **OK** | macroquad is thin; less churn than Bevy/Godot |
| Spike size | **Small** | Packaging spike only; architecture wedge can be in-process |

**Pros**

- Fastest path to “something on Steam” if inspection bar is low.
- Zero draw-port risk; Phase 7 sky/glitter keep working.

**Cons / risks**

- Weakest long-term **Apple / polished store** story.
- Couples ship UX to a research/demo stack; settings UI ceiling is low.
- Temptation to skip the host/client contract entirely (“just ship the
  demo”).

**When D wins:** Owner wants a short Steam experiment and explicitly
defers Apple polish / UI rewrite; macroquad remains both soak and ship
until a later Phase 8b.

---

## Side-by-side

| | A egui+wgpu | B Bevy | C Godot 4 | D macroquad wrap |
|--|-------------|--------|-----------|------------------|
| Keep mass out of client | Best | Needs discipline | OK if host external | OK if split crates |
| Port cost from demo | Medium | High | High | Lowest |
| Steam readiness | High | High | Highest tooling | Medium |
| Apple readiness | High | Medium–High | Medium | Low–Medium |
| UI ceiling | Medium–High | Medium | Highest | Low |
| Agent / soak ops | Easy | Medium | Hard | Easiest |
| Framework churn | Low | High | Medium | Low |
| Fits 2D atlas client | Excellent | Overkill-prone | OK | Already there |

## Recommendation framing (not a decision)

Agent default for **architecture next**, still owner-gated:

1. **Treat D as the soak/dev viewer forever** (or until replaced), not as
   the assumed ship client — unless the owner explicitly chooses D.
2. **Prefer A as the production-compatible default** if the ship target
   is Steam desktop and/or notarized Apple with a 2D atlas + settings
   HUD: hygiene + Metal + lowest lock-in among A/B/C.
3. **Choose B only** if the owner wants Bevy’s presentation runway and
   accepts a hard “sim not in Bevy World” rule in the contract.
4. **Choose C only** if store/editor tooling outweighs dual-runtime cost.

Correct prior shortlist wording: demo settings are **`macroquad::ui`**,
not egui. Stack A is a UI **port**, not a free lunch.

## Suggested spike (after owner pick)

Docs-only contract first ([`PHASE8_SCOPE.md`](PHASE8_SCOPE.md) wedge
steps 2–3). Then a **no-gameplay** spike:

| Pick | Spike deliverable |
|------|-------------------|
| **A** | `winit`+`wgpu` window, egui panel, host tick counter from `wk-voxel` snapshot stub |
| **B** | Bevy app, `bevy_egui` panel, same stub via non-World host handle |
| **C** | Godot scene + Rust extension (or IPC) showing host tick |
| **D** | Notarize/Steam package script for current `wk-voxel-app`; optional in-process host API seam |

Parity gate unchanged: Compact soak **park=0** still green on the host
binary.

## Non-goals (same as Phase 8 scope)

Full multiplayer, console, web WASM ship client, rewriting CA/mass into
the client, abandoning the macroquad soak viewer without an owner call.

## Decision needed (owner)

1. Ship target: Steam / Apple / both / park Phase 8.
2. Stack: **A / B / C / D**.
3. Keep macroquad as: soak-only / soak+ship (D) / retire later.

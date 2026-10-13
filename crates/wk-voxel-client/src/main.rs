//! Phase 8 A client: egui + wgpu / winit over in-process or IPC host.
//!
//! ```text
//! wk-voxel-client                         # in-process Compact
//! wk-voxel-client --dev                   # in-process tiny ring
//! cargo run -p wk-voxel-client --release -- --connect 127.0.0.1:17420
//! cargo run -p wk-voxel-client --release -- --spawn-host            # Compact host child
//! cargo run -p wk-voxel-client --release -- --spawn-host --dev      # tiny host child
//! (package default-run is wk-voxel-client; host bin: --bin wk-voxel-host)
//! ```

use std::env;
use std::sync::Arc;

use egui_wgpu::wgpu;
use egui_wgpu::ScreenDescriptor;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};
use wk_voxel_client::ipc::{RemoteHost, SpawnedHost, DEFAULT_HOST_ADDR};
use wk_voxel_client::{
    ClientCommand, Host, HostInfo, PresentHints, SimSettings, ViewFrame, DEFAULT_SAVE_SLOT,
    VIEW_SCHEMA,
};

enum SimBridge {
    Local(Host),
    Remote(RemoteHost),
    /// Child host process; Drop kills it.
    Spawned(SpawnedHost),
}

fn empty_frame() -> ViewFrame {
    ViewFrame {
        contract_version: VIEW_SCHEMA,
        sim_tick: 0,
        paused: true,
        world_width_cols: 0,
        world_height: 0,
        focus_col: 0,
        view_cols: 0,
        last_step_ms: 0,
        product_step: false,
        sky: wk_voxel_client::SkyPresent {
            sky_rgb: [40, 40, 48],
            day_night_u8: 0,
            phase_u8: 0,
            celestial_local_u8: 0,
            moon_phase_u8: 0,
            season_u8: 0,
            is_day: false,
        },
        sim: SimSettings::default(),
        host: HostInfo::default(),
        present: PresentHints::default(),
        terrain: wk_voxel_client::TerrainStrip {
            origin_col: 0,
            origin_y: 0,
            cols: 0,
            rows: 0,
            rgba: Vec::new(),
        },
        command_acks: Vec::new(),
    }
}

fn pull_remote(r: &mut RemoteHost) -> ViewFrame {
    match r.pull() {
        Ok(f) => f,
        Err(e) => {
            log::error!("remote pull: {e}");
            r.last().cloned().unwrap_or_else(empty_frame)
        }
    }
}

impl SimBridge {
    fn apply_all(&mut self, cmds: Vec<ClientCommand>) {
        match self {
            Self::Local(h) => h.apply_all(cmds),
            Self::Remote(r) => {
                if let Err(e) = r.apply_all(cmds) {
                    log::error!("remote apply: {e}");
                }
            }
            Self::Spawned(s) => {
                if let Err(e) = s.remote.apply_all(cmds) {
                    log::error!("spawned apply: {e}");
                }
            }
        }
    }

    fn frame(&mut self) -> ViewFrame {
        match self {
            Self::Local(h) => h.frame(),
            Self::Remote(r) => pull_remote(r),
            Self::Spawned(s) => pull_remote(&mut s.remote),
        }
    }

    fn quit_requested(&self) -> bool {
        match self {
            Self::Local(h) => h.quit_requested,
            Self::Remote(r) => r.quit_requested(),
            Self::Spawned(s) => s.remote.quit_requested(),
        }
    }

    fn paused(&self) -> bool {
        match self {
            Self::Local(h) => h.paused,
            Self::Remote(r) => r.last().map(|f| f.paused).unwrap_or(false),
            Self::Spawned(s) => s.remote.last().map(|f| f.paused).unwrap_or(false),
        }
    }

    fn focus_col(&self) -> i32 {
        match self {
            Self::Local(h) => h.focus_col,
            Self::Remote(r) => r.last().map(|f| f.focus_col).unwrap_or(0),
            Self::Spawned(s) => s.remote.last().map(|f| f.focus_col).unwrap_or(0),
        }
    }

    fn view_cols(&self) -> i32 {
        match self {
            Self::Local(h) => h.view_cols,
            Self::Remote(r) => r.last().map(|f| f.view_cols).unwrap_or(96),
            Self::Spawned(s) => s.remote.last().map(|f| f.view_cols).unwrap_or(96),
        }
    }

    fn mode_label(&self) -> &'static str {
        match self {
            Self::Local(_) => "in-process",
            Self::Remote(_) => "IPC",
            Self::Spawned(_) => "spawn-host",
        }
    }
}

fn parse_args() -> SimBridge {
    let mut connect: Option<String> = None;
    let mut spawn_host = false;
    let mut dev = false;
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dev" => dev = true,
            "--spawn-host" => spawn_host = true,
            "--connect" => {
                connect = Some(
                    args.next()
                        .unwrap_or_else(|| DEFAULT_HOST_ADDR.to_string()),
                );
            }
            "-h" | "--help" => {
                eprintln!(
                    "wk-voxel-client [--dev] [--connect HOST:PORT] [--spawn-host]\n\
                     default: in-process Compact host\n\
                     --dev: tiny ring (in-process or with --spawn-host)\n\
                     --connect: TCP to existing wk-voxel-host (default {DEFAULT_HOST_ADDR})\n\
                     --spawn-host: launch wk-voxel-host child + connect"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }
    if spawn_host && connect.is_some() {
        eprintln!("--spawn-host and --connect are mutually exclusive");
        std::process::exit(2);
    }
    if spawn_host {
        log::info!("spawning wk-voxel-host (dev={dev})");
        match SpawnedHost::spawn(dev) {
            Ok(s) => {
                log::info!("spawned host pid={}", s.child_id());
                SimBridge::Spawned(s)
            }
            Err(e) => {
                eprintln!("spawn-host: {e}");
                std::process::exit(1);
            }
        }
    } else if let Some(addr) = connect {
        log::info!("connecting to host at {addr}");
        match RemoteHost::connect(&*addr) {
            Ok(r) => SimBridge::Remote(r),
            Err(e) => {
                eprintln!("connect {addr}: {e}");
                std::process::exit(1);
            }
        }
    } else if dev {
        log::info!("in-process Host::for_tests()");
        SimBridge::Local(Host::for_tests())
    } else {
        log::info!("in-process Compact Host::default()");
        SimBridge::Local(Host::default())
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let bridge = parse_args();
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = SpikeApp {
        bridge,
        view: None,
        window: None,
        gpu: None,
        egui_ctx: egui::Context::default(),
        egui_state: None,
        egui_renderer: None,
        pending: Vec::new(),
        terrain_tex: None,
        draft_sim: SimSettings::default(),
        draft_sim_ready: false,
        draft_present: PresentHints::default(),
        draft_present_ready: false,
    };

    event_loop.run_app(&mut app).expect("run");
}

struct GpuState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

struct SpikeApp {
    bridge: SimBridge,
    view: Option<ViewFrame>,
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    pending: Vec<ClientCommand>,
    terrain_tex: Option<egui::TextureHandle>,
    /// Local edit buffer; flushed to host via [`ClientCommand::SetSimSettings`].
    draft_sim: SimSettings,
    draft_sim_ready: bool,
    draft_present: PresentHints,
    draft_present_ready: bool,
}

impl SpikeApp {
    /// Init window + wgpu. Must not panic: on macOS this runs inside AppKit's
    /// `applicationDidFinishLaunching` (`extern "C"` / cannot-unwind).
    fn ensure_gpu(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        if self.gpu.is_some() {
            return Ok(());
        }
        let title = format!(
            "wk-voxel-client (viewer · Phase 8 A · {})",
            self.bridge.mode_label()
        );
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(title)
                        .with_inner_size(LogicalSize::new(1100.0, 640.0)),
                )
                .map_err(|e| format!("create_window: {e}"))?,
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY | wgpu::Backends::GL,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface: {e}"))?;
        // Default: real GPU (Metal on macOS). Force software/GL only when
        // WK_VOXEL_FORCE_FALLBACK_ADAPTER=1 — used for headless Linux spikes.
        // force_fallback_adapter=true made Mac Metal return no adapter and
        // `.expect` aborted inside winit's cannot-unwind launch callback.
        let force_fallback = env::var_os("WK_VOXEL_FORCE_FALLBACK_ADAPTER").is_some();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: force_fallback,
        }))
        .ok_or_else(|| {
            format!(
                "no wgpu adapter (force_fallback={force_fallback}; \
                 unset WK_VOXEL_FORCE_FALLBACK_ADAPTER for Metal/Vulkan)"
            )
        })?;
        let info = adapter.get_info();
        log::info!(
            "wgpu adapter {:?} {:?} {:?}",
            info.name,
            info.backend,
            info.device_type
        );
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("wk-voxel-client"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: Default::default(),
            },
            None,
        ))
        .map_err(|e| format!("request_device: {e}"))?;

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        if caps.formats.is_empty() {
            return Err("surface has no texture formats".into());
        }
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Opaque),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let egui_state = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);

        self.egui_state = Some(egui_state);
        self.egui_renderer = Some(egui_renderer);
        self.gpu = Some(GpuState {
            device,
            queue,
            surface,
            config,
        });
        self.window = Some(window);
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        gpu.config.width = width;
        gpu.config.height = height;
        gpu.surface.configure(&gpu.device, &gpu.config);
    }

    fn pump_host(&mut self) {
        let cmds = std::mem::take(&mut self.pending);
        self.bridge.apply_all(cmds);
        self.view = Some(self.bridge.frame());
    }

    fn upload_terrain(&mut self, view: &ViewFrame) {
        let strip = &view.terrain;
        if strip.cols == 0 || strip.rows == 0 || !strip.byte_len_ok() {
            return;
        }
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [strip.cols as usize, strip.rows as usize],
            &strip.rgba,
        );
        match self.terrain_tex.as_mut() {
            Some(tex) => tex.set(image, egui::TextureOptions::NEAREST),
            None => {
                self.terrain_tex = Some(self.egui_ctx.load_texture(
                    "terrain_strip",
                    image,
                    egui::TextureOptions::NEAREST,
                ));
            }
        }
    }

    fn render(&mut self) {
        if self.bridge.quit_requested() {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };

        let view = self
            .view
            .clone()
            .unwrap_or_else(|| self.bridge.frame());
        if !view.schema_ok() {
            log::error!(
                "ViewFrame schema mismatch: got {} want {}",
                view.contract_version,
                VIEW_SCHEMA
            );
        }
        self.upload_terrain(&view);

        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let Some(egui_state) = self.egui_state.as_mut() else {
            return;
        };
        let Some(egui_renderer) = self.egui_renderer.as_mut() else {
            return;
        };

        let mode = self.bridge.mode_label();
        if !self.draft_sim_ready {
            self.draft_sim = view.sim.clone();
            self.draft_sim_ready = true;
        }
        if !self.draft_present_ready {
            self.draft_present = view.present.clone();
            self.draft_present_ready = true;
        }
        let mut draft = self.draft_sim.clone();
        let mut draft_present = self.draft_present.clone();
        let mut settings_cmd = None;
        let mut present_cmd = None;
        let raw_input = egui_state.take_egui_input(&window);
        let terrain_tex = self.terrain_tex.clone();
        let sky = view.sky.sky_rgb;
        let egui_output = self.egui_ctx.run(raw_input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(egui::Color32::from_rgb(
                    sky[0], sky[1], sky[2],
                )))
                .show(ctx, |ui| {
                    let avail = ui.available_size();
                    if let Some(tex) = &terrain_tex {
                        let tw = view.terrain.cols as f32;
                        let th = view.terrain.rows as f32;
                        let scale = (avail.x / tw).min(avail.y / th).max(1.0);
                        let size = egui::vec2(tw * scale, th * scale);
                        ui.centered_and_justified(|ui| {
                            ui.image((tex.id(), size));
                        });
                    }

                    let local = view.sky.celestial_local_u8 as f32 / 255.0;
                    let body_x = avail.x * (0.12 + 0.76 * local);
                    let body_y = avail.y * 0.12;
                    let (r, g, b, rad) = if view.sky.is_day {
                        (255, 214, 90, 10.0)
                    } else {
                        let illum = view.sky.moon_phase_u8 as f32 / 255.0;
                        let a = (140.0 + 100.0 * (1.0 - (illum - 0.5).abs() * 2.0)) as u8;
                        (a, a, (a as f32 * 1.05).min(255.0) as u8, 7.0)
                    };
                    ui.painter().circle_filled(
                        egui::pos2(body_x, body_y),
                        rad,
                        egui::Color32::from_rgb(r, g, b),
                    );
                });

            egui::Window::new("viewer")
                .default_pos([12.0, 12.0])
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("VIEW_SCHEMA {} · {mode}", view.contract_version));
                    ui.label(format!(
                        "sim_tick {} · step_world {} ms{}",
                        view.sim_tick,
                        view.last_step_ms,
                        if view.product_step { "" } else { " (held)" }
                    ));
                    ui.label(format!(
                        "paused {} · focus {} · view {}",
                        view.paused, view.focus_col, view.view_cols
                    ));
                    ui.label(format!(
                        "world {}×{} · strip {}×{}{}",
                        view.world_width_cols,
                        view.world_height,
                        view.terrain.cols,
                        view.terrain.rows,
                        if view.world_width_cols >= 4096 {
                            " · Compact stream"
                        } else {
                            ""
                        }
                    ));
                    ui.label(format!(
                        "chunks {} live / {} park · org {} / {} · sync −{} +{}",
                        view.host.resident_chunks,
                        view.host.parked_chunks,
                        view.host.live_organisms,
                        view.host.parked_organisms,
                        view.host.last_evicted,
                        view.host.last_reloaded
                    ));
                    ui.label(format!(
                        "sky rgb({},{},{}) · {} · dn {} · season {}",
                        view.sky.sky_rgb[0],
                        view.sky.sky_rgb[1],
                        view.sky.sky_rgb[2],
                        if view.sky.is_day { "day" } else { "night" },
                        view.sky.day_night_u8,
                        view.sky.season_u8
                    ));
                    if let Some(ack) = view.command_acks.last() {
                        ui.label(format!(
                            "ack {} {}",
                            if ack.ok { "ok" } else { "err" },
                            ack.detail
                        ));
                    }
                    ui.separator();
                    ui.label("sim (host)");
                    let mut changed = false;
                    changed |= ui
                        .checkbox(&mut draft.organisms_on, "organisms_on")
                        .changed();
                    changed |= ui.checkbox(&mut draft.evap_on, "evap_on").changed();
                    changed |= ui
                        .checkbox(&mut draft.cond_rain_on, "cond_rain_on")
                        .changed();
                    changed |= ui.checkbox(&mut draft.karst_on, "karst_on").changed();
                    ui.horizontal(|ui| {
                        ui.label("day");
                        changed |= ui
                            .add(egui::DragValue::new(&mut draft.day_ticks).range(1..=10_000u64))
                            .changed();
                        ui.label("night");
                        changed |= ui
                            .add(egui::DragValue::new(&mut draft.night_ticks).range(1..=10_000u64))
                            .changed();
                    });
                    if changed {
                        settings_cmd = Some(draft.clone());
                    }
                    ui.separator();
                    ui.label("present (host)");
                    let mut p_changed = false;
                    p_changed |= ui
                        .checkbox(&mut draft_present.humidity_overlay, "humidity_overlay")
                        .changed();
                    p_changed |= ui
                        .checkbox(&mut draft_present.steam_overlay, "steam_overlay")
                        .changed();
                    p_changed |= ui
                        .checkbox(
                            &mut draft_present.cave_humidity_overlay,
                            "cave_humidity_overlay",
                        )
                        .changed();
                    if p_changed {
                        present_cmd = Some(draft_present.clone());
                    }
                    ui.separator();
                    ui.small("Space pause · N step · ←/→ pan · F5 save · F9 load · R reset · Esc quit");
                    ui.small("client = viewer; host owns step_world + save/load + sim");
                });
        });

        self.draft_sim = draft;
        self.draft_present = draft_present;
        if let Some(s) = settings_cmd {
            self.pending.push(ClientCommand::SetSimSettings(s));
        }
        if let Some(p) = present_cmd {
            self.pending.push(ClientCommand::SetPresentHints(p));
        }

        egui_state.handle_platform_output(&window, egui_output.platform_output);
        let tris = self
            .egui_ctx
            .tessellate(egui_output.shapes, egui_output.pixels_per_point);

        let Ok(surface_tex) = gpu.surface.get_current_texture() else {
            return;
        };
        let view_tex = surface_tex
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let screen = ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point: window.scale_factor() as f32,
        };

        for (id, image_delta) in &egui_output.textures_delta.set {
            egui_renderer.update_texture(&gpu.device, &gpu.queue, *id, image_delta);
        }

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wk-voxel-client frame"),
            });
        let cb = egui_renderer.update_buffers(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &tris,
            &screen,
        );
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("clear+egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view_tex,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: sky[0] as f64 / 255.0,
                                g: sky[1] as f64 / 255.0,
                                b: sky[2] as f64 / 255.0,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            egui_renderer.render(&mut pass, &tris, &screen);
        }
        for id in &egui_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }
        gpu.queue
            .submit(cb.into_iter().chain(std::iter::once(encoder.finish())));
        surface_tex.present();
        window.request_redraw();
    }
}

impl ApplicationHandler for SpikeApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(e) = self.ensure_gpu(event_loop) {
            // Do not panic — macOS runs this under cannot-unwind AppKit code.
            eprintln!("wk-voxel-client: GPU/window init failed: {e}");
            event_loop.exit();
            return;
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        if let Some(window) = &self.window {
            if let Some(state) = self.egui_state.as_mut() {
                let _ = state.on_window_event(window, &event);
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.pending.push(ClientCommand::Quit);
            }
            WindowEvent::Resized(size) => self.resize(size.width, size.height),
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } if key_event.state.is_pressed() => {
                match key_event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape) => {
                        self.pending.push(ClientCommand::Quit);
                    }
                    PhysicalKey::Code(KeyCode::Space) => {
                        self.pending
                            .push(ClientCommand::SetPaused(!self.bridge.paused()));
                    }
                    PhysicalKey::Code(KeyCode::KeyN) => {
                        self.pending.push(ClientCommand::StepOnce);
                    }
                    PhysicalKey::Code(KeyCode::ArrowLeft) => {
                        self.pending.push(ClientCommand::SetCamera {
                            focus_col: self.bridge.focus_col().saturating_sub(8),
                            view_cols: self.bridge.view_cols(),
                        });
                    }
                    PhysicalKey::Code(KeyCode::ArrowRight) => {
                        self.pending.push(ClientCommand::SetCamera {
                            focus_col: self.bridge.focus_col().saturating_add(8),
                            view_cols: self.bridge.view_cols(),
                        });
                    }
                    PhysicalKey::Code(KeyCode::F5) => {
                        self.pending.push(ClientCommand::Save {
                            slot: DEFAULT_SAVE_SLOT.into(),
                        });
                    }
                    PhysicalKey::Code(KeyCode::F9) => {
                        self.pending.push(ClientCommand::Load {
                            slot: DEFAULT_SAVE_SLOT.into(),
                        });
                    }
                    PhysicalKey::Code(KeyCode::KeyR) => {
                        self.pending
                            .push(ClientCommand::ResetWorld { seed: None });
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                self.pump_host();
                if self.bridge.quit_requested() {
                    event_loop.exit();
                    return;
                }
                self.render();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

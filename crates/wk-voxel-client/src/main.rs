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
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use egui_wgpu::wgpu;
use egui_wgpu::ScreenDescriptor;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};
use wk_voxel_client::ipc::{ConnectOpts, PeerRole, RemoteHost, SpawnedHost, DEFAULT_HOST_ADDR};
use wk_voxel_client::{
    ClientCommand, Host, HostInfo, PaintBrush, PresentHints, SimSettings, ViewFrame,
    DEFAULT_SAVE_SLOT, DEFAULT_STEP_PERIOD_MS, MAX_PAINT_RADIUS, MAX_VIEW_COLS, MIN_VIEW_COLS,
    VIEW_SCHEMA,
};

/// Match host auto-step cadence — firm ~20 Hz present / pull.
const PRESENT_PERIOD: Duration = Duration::from_millis(DEFAULT_STEP_PERIOD_MS);
const PAN_COL_STEP: i32 = 16;
const PAN_Y_STEP: i32 = 24;

enum SimBridge {
    Local(Host),
    /// `addr` + connect opts kept for reconnect after a dropped session.
    Remote {
        remote: RemoteHost,
        addr: String,
        opts: ConnectOpts,
    },
    /// Child host process; Drop kills it.
    Spawned(SpawnedHost),
}

#[derive(Clone, Copy, Default)]
struct HeldPan {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
}

enum LaunchIntent {
    Local { dev: bool },
    Connect { addr: String, opts: ConnectOpts },
    Spawn { dev: bool },
}

impl LaunchIntent {
    fn loading_message(&self) -> &'static str {
        match self {
            Self::Local { dev: true } => "Starting tiny in-process host…",
            Self::Local { dev: false } => "Stamping Compact world…",
            Self::Connect { .. } => "Connecting to host…",
            Self::Spawn { dev: true } => "Spawning tiny host…",
            Self::Spawn { dev: false } => "Spawning Compact host (stamp may take a bit)…",
        }
    }

    fn open(self) -> Result<SimBridge, String> {
        match self {
            Self::Local { dev: true } => {
                log::info!("in-process Host::for_tests()");
                Ok(SimBridge::Local(Host::for_tests()))
            }
            Self::Local { dev: false } => {
                log::info!("in-process Compact Host::default()");
                Ok(SimBridge::Local(Host::default()))
            }
            Self::Connect { addr, opts } => {
                log::info!(
                    "connecting to host at {addr} role={:?} auth={}",
                    opts.role_hint,
                    opts.token.is_some()
                );
                RemoteHost::connect_with(&*addr, &opts)
                    .map(|remote| SimBridge::Remote {
                        remote,
                        addr: addr.clone(),
                        opts,
                    })
                    .map_err(|e| format!("connect {addr}: {e}"))
            }
            Self::Spawn { dev } => {
                log::info!("spawning wk-voxel-host (dev={dev})");
                SpawnedHost::spawn(dev)
                    .map(|s| {
                        log::info!("spawned host pid={}", s.child_id());
                        SimBridge::Spawned(s)
                    })
                    .map_err(|e| format!("spawn-host: {e}"))
            }
        }
    }
}

enum Boot {
    Loading {
        rx: Receiver<Result<SimBridge, String>>,
        message: &'static str,
    },
    Ready(SimBridge),
    Failed(String),
}

fn empty_frame() -> ViewFrame {
    ViewFrame {
        contract_version: VIEW_SCHEMA,
        sim_tick: 0,
        paused: true,
        world_width_cols: 0,
        world_height: 0,
        focus_col: 0,
        focus_y: 0,
        view_cols: 0,
        last_step_ms: 0,
        product_step: false,
        sky: wk_voxel_client::SkyPresent {
            sky_rgb: [28, 32, 40],
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
            Self::Remote { remote, .. } => {
                if let Err(e) = remote.apply_all(cmds) {
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
            Self::Remote { remote, addr, opts } => match remote.pull() {
                Ok(f) => f,
                Err(e) => {
                    log::warn!("remote pull: {e} — reconnecting to {addr}");
                    match RemoteHost::connect_with(&**addr, opts) {
                        Ok(r) => {
                            *remote = r;
                            pull_remote(remote)
                        }
                        Err(re) => {
                            log::error!("reconnect {addr}: {re}");
                            remote.last().cloned().unwrap_or_else(empty_frame)
                        }
                    }
                }
            },
            Self::Spawned(s) => pull_remote(&mut s.remote),
        }
    }

    fn quit_requested(&self) -> bool {
        match self {
            Self::Local(h) => h.quit_requested,
            Self::Remote { remote, .. } => remote.quit_requested(),
            Self::Spawned(s) => s.remote.quit_requested(),
        }
    }

    fn role(&self) -> PeerRole {
        match self {
            Self::Local(_) => PeerRole::Operator,
            Self::Remote { remote, .. } => remote.role,
            Self::Spawned(s) => s.remote.role,
        }
    }

    fn is_operator(&self) -> bool {
        matches!(self.role(), PeerRole::Operator)
    }

    fn paused(&self) -> bool {
        match self {
            Self::Local(h) => h.paused,
            Self::Remote { remote, .. } => remote.last().map(|f| f.paused).unwrap_or(false),
            Self::Spawned(s) => s.remote.last().map(|f| f.paused).unwrap_or(false),
        }
    }

    fn focus_col(&self) -> i32 {
        match self {
            Self::Local(h) => h.focus_col,
            Self::Remote { remote, .. } => remote.last().map(|f| f.focus_col).unwrap_or(0),
            Self::Spawned(s) => s.remote.last().map(|f| f.focus_col).unwrap_or(0),
        }
    }

    fn focus_y(&self) -> i32 {
        match self {
            Self::Local(h) => h.focus_y,
            Self::Remote { remote, .. } => remote.last().map(|f| f.focus_y).unwrap_or(0),
            Self::Spawned(s) => s.remote.last().map(|f| f.focus_y).unwrap_or(0),
        }
    }

    fn view_cols(&self) -> i32 {
        match self {
            Self::Local(h) => h.view_cols,
            Self::Remote { remote, .. } => remote.last().map(|f| f.view_cols).unwrap_or(96),
            Self::Spawned(s) => s.remote.last().map(|f| f.view_cols).unwrap_or(96),
        }
    }

    fn set_camera(&self, focus_col: i32, view_cols: i32, focus_y: i32) -> ClientCommand {
        ClientCommand::SetCamera {
            focus_col,
            view_cols: view_cols.clamp(MIN_VIEW_COLS, MAX_VIEW_COLS),
            focus_y,
        }
    }

    fn mode_label(&self) -> &'static str {
        match self {
            Self::Local(_) => "in-process",
            Self::Remote { .. } => "IPC",
            Self::Spawned(_) => "spawn-host",
        }
    }
}

fn parse_intent() -> LaunchIntent {
    let mut connect: Option<String> = None;
    let mut spawn_host = false;
    let mut dev = false;
    let mut token = env::var("WK_VOXEL_HOST_TOKEN").ok().filter(|s| !s.is_empty());
    let mut name: Option<String> = None;
    let mut operator = false;
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dev" => dev = true,
            "--spawn-host" => spawn_host = true,
            "--operator" => operator = true,
            "--connect" => {
                connect = Some(
                    args.next()
                        .unwrap_or_else(|| DEFAULT_HOST_ADDR.to_string()),
                );
            }
            "--token" => {
                token = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--token requires SECRET");
                    std::process::exit(2);
                }));
            }
            "--name" => {
                name = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--name requires LABEL");
                    std::process::exit(2);
                }));
            }
            "-h" | "--help" => {
                eprintln!(
                    "wk-voxel-client [--dev] [--connect HOST:PORT] [--spawn-host]\n\
                     default: in-process Compact host\n\
                     --dev: tiny ring (in-process or with --spawn-host)\n\
                     --connect: TCP to existing wk-voxel-host (default {DEFAULT_HOST_ADDR})\n\
                     --spawn-host: launch wk-voxel-host child + connect\n\
                     --token SECRET: join/ops token (env WK_VOXEL_HOST_TOKEN)\n\
                     --name LABEL: optional peer name\n\
                     --operator: request Operator role (needs host --ops-token)"
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
        LaunchIntent::Spawn { dev }
    } else if let Some(addr) = connect {
        LaunchIntent::Connect {
            addr,
            opts: ConnectOpts {
                token,
                role_hint: if operator {
                    PeerRole::Operator
                } else {
                    PeerRole::Viewer
                },
                client_name: name,
            },
        }
    } else {
        LaunchIntent::Local { dev }
    }
}

fn panel_frame() -> egui::Frame {
    egui::Frame::popup(&egui::Style::default())
        .fill(egui::Color32::from_rgba_unmultiplied(16, 20, 28, 230))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(120, 140, 160, 80),
        ))
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::symmetric(14.0, 12.0))
        .shadow(egui::epaint::Shadow {
            offset: egui::vec2(0.0, 4.0),
            blur: 18.0,
            spread: 0.0,
            color: egui::Color32::from_black_alpha(90),
        })
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let intent = parse_intent();
    let message = intent.loading_message();
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("wk-host-boot".into())
        .spawn(move || {
            let _ = tx.send(intent.open());
        })
        .expect("spawn boot thread");

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + PRESENT_PERIOD));

    let mut app = SpikeApp {
        boot: Boot::Loading { rx, message },
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
        draft_slot: DEFAULT_SAVE_SLOT.into(),
        paint_open: false,
        paint_brush: PaintBrush::Sand,
        paint_radius: 0u8,
        last_terrain_rect: None,
        held: HeldPan::default(),
        hud_open: true,
        load_started: Instant::now(),
        next_present: Instant::now(),
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
    boot: Boot,
    view: Option<ViewFrame>,
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    pending: Vec<ClientCommand>,
    terrain_tex: Option<egui::TextureHandle>,
    draft_sim: SimSettings,
    draft_sim_ready: bool,
    draft_present: PresentHints,
    draft_present_ready: bool,
    draft_slot: String,
    paint_open: bool,
    paint_brush: PaintBrush,
    paint_radius: u8,
    /// Cover-scale terrain blit rect from last frame (for paint hit-test).
    last_terrain_rect: Option<egui::Rect>,
    held: HeldPan,
    hud_open: bool,
    load_started: Instant,
    next_present: Instant,
}

impl SpikeApp {
    fn bridge_mut(&mut self) -> Option<&mut SimBridge> {
        match &mut self.boot {
            Boot::Ready(b) => Some(b),
            _ => None,
        }
    }

    fn bridge(&self) -> Option<&SimBridge> {
        match &self.boot {
            Boot::Ready(b) => Some(b),
            _ => None,
        }
    }

    fn poll_boot(&mut self) {
        let Boot::Loading { rx, message } = &self.boot else {
            return;
        };
        let message = *message;
        match rx.try_recv() {
            Ok(Ok(bridge)) => {
                if let Some(w) = &self.window {
                    w.set_title(&format!(
                        "wk-voxel · {} · Phase 8",
                        bridge.mode_label()
                    ));
                }
                log::info!("boot ready ({})", bridge.mode_label());
                self.boot = Boot::Ready(bridge);
            }
            Ok(Err(e)) => {
                eprintln!("wk-voxel-client: {e}");
                self.boot = Boot::Failed(e);
            }
            Err(mpsc::TryRecvError::Empty) => {
                let _ = message;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.boot = Boot::Failed("boot thread exited without result".into());
            }
        }
    }

    fn flush_held_pan(&mut self) {
        let Some(bridge) = self.bridge() else {
            return;
        };
        let mut d_col = 0i32;
        let mut d_y = 0i32;
        if self.held.left {
            d_col -= PAN_COL_STEP;
        }
        if self.held.right {
            d_col += PAN_COL_STEP;
        }
        if self.held.up {
            d_y += PAN_Y_STEP;
        }
        if self.held.down {
            d_y -= PAN_Y_STEP;
        }
        if d_col == 0 && d_y == 0 {
            return;
        }
        let cmd = bridge.set_camera(
            bridge.focus_col().saturating_add(d_col),
            bridge.view_cols(),
            bridge.focus_y().saturating_add(d_y),
        );
        self.pending.push(cmd);
    }

    /// Init window + wgpu. Must not panic: on macOS this runs inside AppKit's
    /// `applicationDidFinishLaunching` (`extern "C"` / cannot-unwind).
    fn ensure_gpu(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        if self.gpu.is_some() {
            return Ok(());
        }
        let title = "wk-voxel · starting…".to_string();
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
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Opaque),
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
        self.flush_held_pan();
        let cmds = std::mem::take(&mut self.pending);
        let Some(bridge) = self.bridge_mut() else {
            return;
        };
        bridge.apply_all(cmds);
        self.view = Some(bridge.frame());
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

    fn schedule_present(&mut self, event_loop: &ActiveEventLoop) {
        self.next_present = Instant::now() + PRESENT_PERIOD;
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_present));
    }

    fn render_boot_or_play(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_boot();
        match &self.boot {
            Boot::Loading { .. } | Boot::Failed(_) => self.render_splash(event_loop),
            Boot::Ready(_) => {
                self.pump_host();
                if self.bridge().is_some_and(|b| b.quit_requested()) {
                    event_loop.exit();
                    return;
                }
                self.render_play(event_loop);
            }
        }
    }

    fn render_splash(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let Some(egui_state) = self.egui_state.as_mut() else {
            return;
        };
        let Some(egui_renderer) = self.egui_renderer.as_mut() else {
            return;
        };

        let (title, detail, failed) = match &self.boot {
            Boot::Loading { message, .. } => ("wk-voxel", *message, false),
            Boot::Failed(e) => ("wk-voxel", e.as_str(), true),
            Boot::Ready(_) => return,
        };
        let elapsed = self.load_started.elapsed().as_secs_f32();

        let raw_input = egui_state.take_egui_input(&window);
        let egui_output = self.egui_ctx.run(raw_input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(egui::Color32::from_rgb(14, 18, 24)))
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.28);
                        ui.label(
                            egui::RichText::new(title)
                                .size(42.0)
                                .color(egui::Color32::from_rgb(230, 236, 242))
                                .strong(),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Phase 8 viewer")
                                .size(16.0)
                                .color(egui::Color32::from_rgb(140, 160, 180)),
                        );
                        ui.add_space(28.0);
                        if failed {
                            ui.label(
                                egui::RichText::new(detail)
                                    .size(15.0)
                                    .color(egui::Color32::from_rgb(240, 120, 110)),
                            );
                            ui.add_space(12.0);
                            ui.label(
                                egui::RichText::new("Esc to quit")
                                    .size(13.0)
                                    .color(egui::Color32::from_rgb(120, 130, 140)),
                            );
                        } else {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(16.0));
                                ui.label(
                                    egui::RichText::new(detail)
                                        .size(15.0)
                                        .color(egui::Color32::from_rgb(190, 200, 210)),
                                );
                            });
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new(format!("{elapsed:.1}s"))
                                    .size(12.0)
                                    .color(egui::Color32::from_rgb(100, 110, 120)),
                            );
                        }
                    });
                });
        });

        egui_state.handle_platform_output(&window, egui_output.platform_output);
        let tris = self
            .egui_ctx
            .tessellate(egui_output.shapes, egui_output.pixels_per_point);
        let Ok(surface_tex) = gpu.surface.get_current_texture() else {
            self.schedule_present(event_loop);
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
                label: Some("splash"),
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
                    label: Some("splash"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view_tex,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 14.0 / 255.0,
                                g: 18.0 / 255.0,
                                b: 24.0 / 255.0,
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
        self.schedule_present(event_loop);
    }

    fn render_play(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some(bridge) = self.bridge() else {
            return;
        };
        let mode = bridge.mode_label();

        let view = self
            .view
            .clone()
            .unwrap_or_else(empty_frame);
        if !view.schema_ok() && view.world_width_cols > 0 {
            log::error!(
                "ViewFrame schema mismatch: got {} want {}",
                view.contract_version,
                VIEW_SCHEMA
            );
        }
        self.upload_terrain(&view);

        if !self.draft_sim_ready {
            self.draft_sim = view.sim.clone();
            self.draft_sim_ready = true;
        }
        if !self.draft_present_ready {
            self.draft_present = view.present.clone();
            self.draft_present_ready = true;
        }
        let is_operator = self
            .bridge()
            .map(|b| b.is_operator())
            .unwrap_or(true);
        let role = self
            .bridge()
            .map(|b| b.role())
            .unwrap_or(PeerRole::Operator);
        let mut draft = self.draft_sim.clone();
        let mut draft_present = self.draft_present.clone();
        let mut draft_slot = self.draft_slot.clone();
        let mut paint_open = self.paint_open;
        let mut paint_brush = self.paint_brush;
        let mut paint_radius = self.paint_radius;
        let mut settings_cmd = None;
        let mut present_cmd = None;
        let mut op_cmd: Option<ClientCommand> = None;
        let mut paint_cmd: Option<ClientCommand> = None;
        let mut hud_open = self.hud_open;
        let mut terrain_rect_out: Option<egui::Rect> = None;

        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let Some(egui_state) = self.egui_state.as_mut() else {
            return;
        };
        let Some(egui_renderer) = self.egui_renderer.as_mut() else {
            return;
        };
        let raw_input = egui_state.take_egui_input(&window);
        let terrain_tex = self.terrain_tex.clone();
        let sky = view.sky.sky_rgb;
        let egui_output = self.egui_ctx.run(raw_input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(egui::Color32::from_rgb(
                    sky[0], sky[1], sky[2],
                )))
                .show(ctx, |ui| {
                    let rect = ui.available_rect_before_wrap();
                    // Cover-scale: fill the window (no sky letterbox bars).
                    // Strip is wider aspect than a typical window, so we crop
                    // a little left/right instead of banding top/bottom.
                    if let Some(tex) = &terrain_tex {
                        let tw = view.terrain.cols as f32;
                        let th = view.terrain.rows as f32;
                        let scale = (rect.width() / tw).max(rect.height() / th);
                        let size = egui::vec2(tw * scale, th * scale);
                        let image_rect = egui::Rect::from_center_size(rect.center(), size);
                        terrain_rect_out = Some(image_rect);
                        let uv = egui::Rect::from_min_max(
                            egui::pos2(0.0, 0.0),
                            egui::pos2(1.0, 1.0),
                        );
                        ui.set_clip_rect(rect);
                        ui.painter().image(
                            tex.id(),
                            image_rect,
                            uv,
                            egui::Color32::WHITE,
                        );
                    }

                    let local = view.sky.celestial_local_u8 as f32 / 255.0;
                    let body_x = rect.left() + rect.width() * (0.12 + 0.76 * local);
                    let body_y = rect.top() + rect.height() * 0.12;
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

            egui::Window::new("wk-voxel")
                .default_pos([16.0, 16.0])
                .resizable(false)
                .collapsible(true)
                .open(&mut hud_open)
                .frame(panel_frame())
                .show(ctx, |ui| {
                    ui.style_mut().spacing.item_spacing = egui::vec2(6.0, 5.0);
                    ui.label(
                        egui::RichText::new(format!("{mode} · schema {}", view.contract_version))
                            .small()
                            .color(egui::Color32::from_rgb(140, 160, 175)),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "tick {} · {} ms/step{}",
                            view.sim_tick,
                            view.last_step_ms,
                            if view.product_step { "" } else { " · held" }
                        ))
                        .strong()
                        .color(egui::Color32::from_rgb(230, 236, 242)),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {:?} · focus {},{} · view {}",
                            if view.paused { "paused" } else { "running" },
                            role,
                            view.focus_col,
                            view.focus_y,
                            view.view_cols
                        ))
                        .color(egui::Color32::from_rgb(200, 210, 220)),
                    );

                    egui::CollapsingHeader::new("world")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.label(format!(
                                "{}×{} · strip {}×{}",
                                view.world_width_cols,
                                view.world_height,
                                view.terrain.cols,
                                view.terrain.rows
                            ));
                            ui.label(format!(
                                "chunks {} / {} park · org {} / {}",
                                view.host.resident_chunks,
                                view.host.parked_chunks,
                                view.host.live_organisms,
                                view.host.parked_organisms
                            ));
                            ui.label(format!(
                                "{} · season {}",
                                if view.sky.is_day { "day" } else { "night" },
                                view.sky.season_u8
                            ));
                            if let Some(ack) = view.command_acks.last() {
                                ui.label(format!(
                                    "ack {} — {}",
                                    if ack.ok { "ok" } else { "err" },
                                    ack.detail
                                ));
                            }
                        });

                    if is_operator {
                        egui::CollapsingHeader::new("sim")
                            .default_open(true)
                            .show(ui, |ui| {
                                let mut changed = false;
                                changed |=
                                    ui.checkbox(&mut draft.organisms_on, "organisms").changed();
                                changed |=
                                    ui.checkbox(&mut draft.evap_on, "evaporation").changed();
                                changed |=
                                    ui.checkbox(&mut draft.cond_rain_on, "rain").changed();
                                changed |= ui.checkbox(&mut draft.karst_on, "karst").changed();
                                ui.horizontal(|ui| {
                                    ui.label("day");
                                    changed |= ui
                                        .add(
                                            egui::DragValue::new(&mut draft.day_ticks)
                                                .range(1..=10_000u64),
                                        )
                                        .changed();
                                    ui.label("night");
                                    changed |= ui
                                        .add(
                                            egui::DragValue::new(&mut draft.night_ticks)
                                                .range(1..=10_000u64),
                                        )
                                        .changed();
                                });
                                if changed {
                                    settings_cmd = Some(draft.clone());
                                }
                            });

                        egui::CollapsingHeader::new("operator")
                            .default_open(true)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label("slot");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut draft_slot)
                                            .desired_width(80.0),
                                    );
                                });
                                ui.horizontal(|ui| {
                                    if ui.button("Save").clicked() {
                                        let slot = draft_slot.trim().to_string();
                                        if !slot.is_empty() {
                                            op_cmd = Some(ClientCommand::Save { slot });
                                        }
                                    }
                                    if ui.button("Load").clicked() {
                                        let slot = draft_slot.trim().to_string();
                                        if !slot.is_empty() {
                                            op_cmd = Some(ClientCommand::Load { slot });
                                        }
                                    }
                                    if ui.button("Reset").clicked() {
                                        op_cmd =
                                            Some(ClientCommand::ResetWorld { seed: None });
                                    }
                                });
                                ui.checkbox(&mut paint_open, "paint (F3)");
                                if paint_open {
                                    ui.add(
                                        egui::Slider::new(
                                            &mut paint_radius,
                                            0..=MAX_PAINT_RADIUS as u8,
                                        )
                                        .text("radius"),
                                    );
                                    egui::ComboBox::from_label("brush")
                                        .selected_text(paint_brush.label())
                                        .show_ui(ui, |ui| {
                                            for b in PaintBrush::ALL {
                                                ui.selectable_value(
                                                    &mut paint_brush,
                                                    b,
                                                    b.label(),
                                                );
                                            }
                                        });
                                    ui.label(
                                        egui::RichText::new("click terrain to paint")
                                            .small()
                                            .color(egui::Color32::from_rgb(140, 160, 120)),
                                    );
                                }
                            });
                    } else {
                        ui.label(
                            egui::RichText::new("viewer — sim / save gated (need --operator)")
                                .small()
                                .color(egui::Color32::from_rgb(160, 140, 110)),
                        );
                    }

                    egui::CollapsingHeader::new("overlays")
                        .default_open(false)
                        .show(ui, |ui| {
                            let mut p_changed = false;
                            p_changed |= ui
                                .checkbox(&mut draft_present.humidity_overlay, "humidity")
                                .changed();
                            p_changed |= ui
                                .checkbox(&mut draft_present.steam_overlay, "steam")
                                .changed();
                            p_changed |= ui
                                .checkbox(&mut draft_present.cave_humidity_overlay, "cave humidity")
                                .changed();
                            if p_changed {
                                present_cmd = Some(draft_present.clone());
                            }
                        });

                    ui.add_space(4.0);
                    let hints = if is_operator {
                        "arrows pan · [ ] · Space/N · F3 paint · F5/F9 · R · Esc · H"
                    } else {
                        "arrows pan · [ ] width · Space pause · N step · Esc · H hud"
                    };
                    ui.label(
                        egui::RichText::new(hints)
                            .small()
                            .color(egui::Color32::from_rgb(110, 125, 140)),
                    );
                });

            // Paint click: terrain under pointer, not when UI wants the click.
            if is_operator
                && paint_open
                && !ctx.wants_pointer_input()
                && ctx.input(|i| i.pointer.primary_down())
            {
                if let (Some(pos), Some(image_rect)) =
                    (ctx.pointer_interact_pos(), terrain_rect_out)
                {
                    if image_rect.contains(pos) && view.terrain.cols > 0 && view.terrain.rows > 0
                    {
                        let u = ((pos.x - image_rect.min.x) / image_rect.width())
                            .clamp(0.0, 0.999);
                        let v = ((pos.y - image_rect.min.y) / image_rect.height())
                            .clamp(0.0, 0.999);
                        let col = view.terrain.origin_col
                            + (u * view.terrain.cols as f32).floor() as i32;
                        let y = view.terrain.origin_y
                            + (view.terrain.rows as i32 - 1)
                            - (v * view.terrain.rows as f32).floor() as i32;
                        paint_cmd = Some(ClientCommand::PaintTerrain {
                            col,
                            y,
                            brush: paint_brush,
                            radius: paint_radius,
                        });
                    }
                }
            }
        });

        self.hud_open = hud_open;
        self.draft_sim = draft;
        self.draft_present = draft_present;
        self.draft_slot = draft_slot;
        self.paint_open = paint_open;
        self.paint_brush = paint_brush;
        self.paint_radius = paint_radius;
        self.last_terrain_rect = terrain_rect_out;
        if let Some(s) = settings_cmd {
            if is_operator {
                self.pending.push(ClientCommand::SetSimSettings(s));
            }
        }
        if let Some(p) = present_cmd {
            self.pending.push(ClientCommand::SetPresentHints(p));
        }
        if let Some(c) = op_cmd {
            self.pending.push(c);
        }
        if let Some(c) = paint_cmd {
            self.pending.push(c);
        }

        egui_state.handle_platform_output(&window, egui_output.platform_output);
        let tris = self
            .egui_ctx
            .tessellate(egui_output.shapes, egui_output.pixels_per_point);

        let Ok(surface_tex) = gpu.surface.get_current_texture() else {
            self.schedule_present(event_loop);
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
        self.schedule_present(event_loop);
    }
}

impl ApplicationHandler for SpikeApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(e) = self.ensure_gpu(event_loop) {
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
                if matches!(self.boot, Boot::Ready(_)) {
                    self.pending.push(ClientCommand::Quit);
                } else {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => self.resize(size.width, size.height),
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                let pressed = key_event.state == ElementState::Pressed;
                let physical = key_event.physical_key;
                // Track arrows for diagonal hold (press + release).
                match physical {
                    PhysicalKey::Code(KeyCode::ArrowLeft) => self.held.left = pressed,
                    PhysicalKey::Code(KeyCode::ArrowRight) => self.held.right = pressed,
                    PhysicalKey::Code(KeyCode::ArrowUp) => self.held.up = pressed,
                    PhysicalKey::Code(KeyCode::ArrowDown) => self.held.down = pressed,
                    _ => {}
                }
                if !pressed || key_event.repeat {
                    return;
                }
                // One-shot keys.
                match physical {
                    PhysicalKey::Code(KeyCode::Escape) => {
                        if matches!(self.boot, Boot::Ready(_)) {
                            self.pending.push(ClientCommand::Quit);
                        } else {
                            event_loop.exit();
                        }
                    }
                    PhysicalKey::Code(KeyCode::KeyH) => {
                        self.hud_open = !self.hud_open;
                    }
                    PhysicalKey::Code(KeyCode::F3) => {
                        if self.bridge().map(|b| b.is_operator()).unwrap_or(false) {
                            self.paint_open = !self.paint_open;
                        }
                    }
                    PhysicalKey::Code(KeyCode::Space) => {
                        if let Some(b) = self.bridge() {
                            self.pending
                                .push(ClientCommand::SetPaused(!b.paused()));
                        }
                    }
                    PhysicalKey::Code(KeyCode::KeyN) => {
                        self.pending.push(ClientCommand::StepOnce);
                    }
                    PhysicalKey::Code(KeyCode::BracketRight) => {
                        if let Some(b) = self.bridge() {
                            self.pending.push(b.set_camera(
                                b.focus_col(),
                                b.view_cols().saturating_add(80),
                                b.focus_y(),
                            ));
                        }
                    }
                    PhysicalKey::Code(KeyCode::BracketLeft) => {
                        if let Some(b) = self.bridge() {
                            self.pending.push(b.set_camera(
                                b.focus_col(),
                                b.view_cols().saturating_sub(80),
                                b.focus_y(),
                            ));
                        }
                    }
                    PhysicalKey::Code(KeyCode::F5) => {
                        if self.bridge().map(|b| b.is_operator()).unwrap_or(false) {
                            let slot = self.draft_slot.trim().to_string();
                            let slot = if slot.is_empty() {
                                DEFAULT_SAVE_SLOT.into()
                            } else {
                                slot
                            };
                            self.pending.push(ClientCommand::Save { slot });
                        }
                    }
                    PhysicalKey::Code(KeyCode::F9) => {
                        if self.bridge().map(|b| b.is_operator()).unwrap_or(false) {
                            let slot = self.draft_slot.trim().to_string();
                            let slot = if slot.is_empty() {
                                DEFAULT_SAVE_SLOT.into()
                            } else {
                                slot
                            };
                            self.pending.push(ClientCommand::Load { slot });
                        }
                    }
                    PhysicalKey::Code(KeyCode::KeyR) => {
                        if self.bridge().map(|b| b.is_operator()).unwrap_or(false) {
                            self.pending
                                .push(ClientCommand::ResetWorld { seed: None });
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                self.render_boot_or_play(event_loop);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if Instant::now() >= self.next_present {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_present));
    }
}

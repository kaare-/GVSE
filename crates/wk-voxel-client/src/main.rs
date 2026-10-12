//! Phase 8 A client: egui + wgpu / winit over an in-process [`Host`].
//!
//! Terrain strip from the host is shown as an egui `ColorImage` (wgpu-backed).
//! Sky / glitter / full palette parity are later wedges.

use std::sync::Arc;

use egui_wgpu::wgpu;
use egui_wgpu::ScreenDescriptor;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};
use wk_voxel_client::{ClientCommand, Host, ViewFrame, VIEW_SCHEMA};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = SpikeApp {
        host: Host::default(),
        view: None,
        window: None,
        gpu: None,
        egui_ctx: egui::Context::default(),
        egui_state: None,
        egui_renderer: None,
        pending: Vec::new(),
        terrain_tex: None,
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
    host: Host,
    view: Option<ViewFrame>,
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    pending: Vec<ClientCommand>,
    terrain_tex: Option<egui::TextureHandle>,
}

impl SpikeApp {
    fn ensure_gpu(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("wk-voxel-client (viewer · Phase 8 A)")
                        .with_inner_size(LogicalSize::new(1100.0, 640.0)),
                )
                .expect("window"),
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY | wgpu::Backends::GL,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: true,
        }))
        .expect("adapter");
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
        .expect("device");

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
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
            alpha_mode: caps.alpha_modes[0],
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
        self.host.apply_all(cmds);
        self.view = Some(self.host.frame());
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
        if self.host.quit_requested {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };

        let view = self.view.clone().unwrap_or_else(|| self.host.frame());
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

                    // Simple sun/moon marker along the top of the strip.
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
                    ui.label(format!("VIEW_SCHEMA {}", view.contract_version));
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
                        "world {}×{} · strip {}×{}",
                        view.world_width_cols,
                        view.world_height,
                        view.terrain.cols,
                        view.terrain.rows
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
                    ui.separator();
                    ui.small("Space pause · N step · ←/→ pan · Esc quit");
                    ui.small("client = viewer; host owns step_world");
                });
        });

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
        self.ensure_gpu(event_loop);
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
                            .push(ClientCommand::SetPaused(!self.host.paused));
                    }
                    PhysicalKey::Code(KeyCode::KeyN) => {
                        self.pending.push(ClientCommand::StepOnce);
                    }
                    PhysicalKey::Code(KeyCode::ArrowLeft) => {
                        self.pending.push(ClientCommand::SetCamera {
                            focus_col: self.host.focus_col.saturating_sub(8),
                            view_cols: self.host.view_cols,
                        });
                    }
                    PhysicalKey::Code(KeyCode::ArrowRight) => {
                        self.pending.push(ClientCommand::SetCamera {
                            focus_col: self.host.focus_col.saturating_add(8),
                            view_cols: self.host.view_cols,
                        });
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                self.pump_host();
                if self.host.quit_requested {
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

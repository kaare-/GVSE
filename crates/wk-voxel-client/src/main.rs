//! Empty-window Phase 8 spike: egui + wgpu / winit over an in-process [`Host`].
//!
//! No terrain atlas yet — clear color + egui HUD showing `ViewFrame.sim_tick`.

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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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
                        .with_title("wk-voxel-client spike (Phase 8 A)")
                        .with_inner_size(LogicalSize::new(960.0, 540.0)),
                )
                .expect("window"),
        );

        // PRIMARY + GL: desktop Vulkan/Metal/DX when present; Mesa GL under Xvfb.
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
            .find(|f| f.is_srgb())
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
        if self.host.quit_requested {
            if let Some(w) = &self.window {
                w.set_visible(false);
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
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let Some(egui_state) = self.egui_state.as_mut() else {
            return;
        };
        let Some(egui_renderer) = self.egui_renderer.as_mut() else {
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

        let raw_input = egui_state.take_egui_input(&window);
        let egui_output = self.egui_ctx.run(raw_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Phase 8 A spike — egui + wgpu / winit");
                ui.separator();
                ui.label(format!("VIEW_SCHEMA {}", view.contract_version));
                ui.label(format!("sim_tick {}", view.sim_tick));
                ui.label(format!(
                    "paused {} · focus_col {} · view_cols {}",
                    view.paused, view.focus_col, view.view_cols
                ));
                ui.label(format!(
                    "world {}×{} (Compact-shaped stub)",
                    view.world_width_cols, view.world_height
                ));
                ui.separator();
                ui.label("Space — pause/resume · N — step once · ←/→ — pan focus · Esc — quit");
                if !view.command_acks.is_empty() {
                    ui.separator();
                    ui.label("acks:");
                    for ack in &view.command_acks {
                        ui.small(format!(
                            "#{} {} — {}",
                            ack.id,
                            if ack.ok { "ok" } else { "reject" },
                            ack.detail
                        ));
                    }
                }
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
                                r: 0.08,
                                g: 0.10,
                                b: 0.14,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                // egui-wgpu 0.29 asks for RenderPass<'static>.
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

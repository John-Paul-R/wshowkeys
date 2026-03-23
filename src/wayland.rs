use crate::render::{self, BufferPool};
use std::os::fd::OwnedFd;
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_registry, wl_seat, wl_shm,
    wl_shm_pool, wl_surface,
};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};

/// Surface lifecycle: prevents buffer attachment before configure.
#[derive(Debug, Clone, Copy)]
pub enum SurfaceGeometry {
    Unconfigured,
    Configured { width: u32, height: u32 },
}

/// Frame pacing state: prevents flooding the compositor.
#[derive(Debug, Clone, Copy)]
pub enum FrameState {
    Idle,
    Pending { dirty: bool },
}

pub struct Output {
    pub output: wl_output::WlOutput,
    pub scale: i32,
    pub subpixel: wl_output::Subpixel,
}

pub struct WskState {
    // Wayland globals
    pub compositor: Option<wl_compositor::WlCompositor>,
    pub shm: Option<wl_shm::WlShm>,
    pub seat: Option<wl_seat::WlSeat>,
    pub layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,

    // Surface state
    pub surface: Option<wl_surface::WlSurface>,
    pub layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    pub geometry: SurfaceGeometry,
    pub frame_state: FrameState,
    pub buffer_pool: BufferPool,

    // Outputs
    pub outputs: Vec<Output>,
    pub current_output: Option<usize>,

    // Keyboard (for keymap delivery)
    pub keyboard: Option<wl_keyboard::WlKeyboard>,
    pub keymap_update: Option<(OwnedFd, u32)>,

    pub running: bool,
    pub needs_render: bool,
}

impl WskState {
    pub fn new() -> Self {
        Self {
            compositor: None,
            shm: None,
            seat: None,
            layer_shell: None,
            surface: None,
            layer_surface: None,
            geometry: SurfaceGeometry::Unconfigured,
            frame_state: FrameState::Idle,
            buffer_pool: BufferPool::new(),
            outputs: Vec::new(),
            current_output: None,
            keyboard: None,
            keymap_update: None,
            running: true,
            needs_render: false,
        }
    }

    pub fn current_scale(&self) -> i32 {
        self.current_output
            .and_then(|i| self.outputs.get(i))
            .map(|o| o.scale)
            .unwrap_or(1)
    }

    /// Mark the surface as needing a redraw.
    pub fn set_dirty(&mut self, _qh: &QueueHandle<Self>) {
        match self.frame_state {
            FrameState::Pending { .. } => {
                self.frame_state = FrameState::Pending { dirty: true };
            }
            FrameState::Idle => {
                self.needs_render = true;
            }
        }
    }

    /// Perform a render cycle. Called from the main loop when needs_render is set.
    pub fn render_frame(
        &mut self,
        keys: &[crate::input::Keypress],
        font: &str,
        foreground: u32,
        specialfg: u32,
        background: u32,
        qh: &QueueHandle<Self>,
    ) {
        self.needs_render = false;

        let surface = match self.surface.as_ref() {
            Some(s) => s,
            None => return,
        };
        let layer_surface = match self.layer_surface.as_ref() {
            Some(ls) => ls,
            None => return,
        };

        let scale = self.current_scale();

        let (recorder, pixel_w, pixel_h) =
            render::measure_and_render(keys, font, scale, foreground, specialfg, background);

        let logical_w = if scale > 0 { pixel_w / scale as u32 } else { pixel_w };
        let logical_h = if scale > 0 { pixel_h / scale as u32 } else { pixel_h };

        let needs_reconfigure = match self.geometry {
            SurfaceGeometry::Unconfigured => true,
            SurfaceGeometry::Configured { width, height } => {
                width != logical_w || height != logical_h
            }
        };

        if needs_reconfigure {
            self.geometry = SurfaceGeometry::Unconfigured;
            if pixel_w == 0 || pixel_h == 0 {
                surface.attach(None, 0, 0);
            } else {
                layer_surface.set_size(logical_w, logical_h);
            }
            surface.commit();
            return;
        }

        if pixel_h == 0 {
            return;
        }

        let shm = match self.shm.as_ref() {
            Some(s) => s,
            None => return,
        };

        let buf_idx =
            match self.buffer_pool.get_buffer(shm, pixel_w, pixel_h, qh) {
                Some(i) => i,
                None => return,
            };

        {
            let buf = self.buffer_pool.buffer(buf_idx).unwrap();
            render::replay_to_buffer(&recorder, &buf.surface);

            surface.set_buffer_scale(scale);
            surface.attach(Some(&buf.buffer), 0, 0);
            surface.damage_buffer(0, 0, pixel_w as i32, pixel_h as i32);
        }

        let _cb = surface.frame(qh, ());
        self.frame_state = FrameState::Pending { dirty: false };
        surface.commit();
    }
}

// -- Registry --

impl Dispatch<wl_registry::WlRegistry, ()> for WskState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version: _,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor =
                        Some(registry.bind::<wl_compositor::WlCompositor, _, _>(name, 4, qh, ()));
                }
                "wl_shm" => {
                    state.shm =
                        Some(registry.bind::<wl_shm::WlShm, _, _>(name, 1, qh, ()));
                }
                "wl_seat" => {
                    state.seat =
                        Some(registry.bind::<wl_seat::WlSeat, _, _>(name, 5, qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell =
                        Some(registry.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(
                            name, 1, qh, (),
                        ));
                }
                "wl_output" => {
                    let output = registry.bind::<wl_output::WlOutput, _, _>(name, 3, qh, ());
                    state.outputs.push(Output {
                        output,
                        scale: 1,
                        subpixel: wl_output::Subpixel::Unknown,
                    });
                }
                _ => {}
            }
        }
    }
}

// -- Layer surface --

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for WskState {
    fn event(
        _: &mut Self,
        _: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for WskState {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surface.ack_configure(serial);
                state.geometry = SurfaceGeometry::Configured { width, height };
                state.set_dirty(qh);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.running = false;
            }
            _ => {}
        }
    }
}

// -- Surface --

impl Dispatch<wl_surface::WlSurface, ()> for WskState {
    fn event(
        state: &mut Self,
        _surface: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::Enter { output } = event {
            for (i, o) in state.outputs.iter().enumerate() {
                if o.output == output {
                    state.current_output = Some(i);
                    return;
                }
            }
        }
    }
}

// -- Frame callback --

impl Dispatch<wl_callback::WlCallback, ()> for WskState {
    fn event(
        state: &mut Self,
        _cb: &wl_callback::WlCallback,
        _event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let was_dirty = matches!(state.frame_state, FrameState::Pending { dirty: true });
        state.frame_state = FrameState::Idle;
        if was_dirty {
            state.needs_render = true;
        }
    }
}

// -- Seat + Keyboard --

impl Dispatch<wl_seat::WlSeat, ()> for WskState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if state.keyboard.is_none()
                && caps.contains(wl_seat::Capability::Keyboard)
            {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for WskState {
    fn event(
        state: &mut Self,
        _kb: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Keymap { format, fd, size } = event {
            if let WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) = format {
                state.keymap_update = Some((fd, size));
            }
        }
    }
}

// -- Output --

impl Dispatch<wl_output::WlOutput, ()> for WskState {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(entry) = state.outputs.iter_mut().find(|o| o.output == *output) else {
            return;
        };

        match event {
            wl_output::Event::Geometry { subpixel, .. } => {
                if let WEnum::Value(sp) = subpixel {
                    entry.subpixel = sp;
                }
            }
            wl_output::Event::Scale { factor } => {
                entry.scale = factor;
            }
            _ => {}
        }
    }
}

// -- Buffer --

impl Dispatch<wl_buffer::WlBuffer, ()> for WskState {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.buffer_pool.release(buffer);
        }
    }
}

// -- Compositor + SHM (no interesting events) --

delegate_noop!(WskState: ignore wl_compositor::WlCompositor);
delegate_noop!(WskState: ignore wl_shm::WlShm);
delegate_noop!(WskState: ignore wl_shm_pool::WlShmPool);

use std::os::fd::AsRawFd;

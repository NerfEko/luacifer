use std::os::unix::io::AsFd;

use tempfile::tempfile;
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, delegate_noop,
    protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool,
        wl_surface,
    },
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use super::wayland_helpers::{deadline_from_hold, deadline_reached, draw_probe_buffer};

pub fn run(title: &str, hold_ms: u64) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = XdgProbeState {
        title: title.to_string(),
        running: true,
        base_surface: None,
        buffer: None,
        wm_base: None,
        xdg_surface: None,
        configured: false,
    };

    let deadline = deadline_from_hold(hold_ms);
    while state.running && !deadline_reached(deadline) {
        event_queue.blocking_dispatch(&mut state)?;
    }
    Ok(())
}

struct XdgProbeState {
    title: String,
    running: bool,
    base_surface: Option<wl_surface::WlSurface>,
    buffer: Option<wl_buffer::WlBuffer>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    xdg_surface: Option<(xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel)>,
    configured: bool,
}

impl XdgProbeState {
    fn init_xdg_surface(&mut self, qh: &QueueHandle<Self>) {
        if self.xdg_surface.is_some() {
            return;
        }
        let wm_base = self.wm_base.as_ref().expect("wm base");
        let base_surface = self.base_surface.as_ref().expect("base surface");
        let xdg_surface = wm_base.get_xdg_surface(base_surface, qh, ());
        let toplevel = xdg_surface.get_toplevel(qh, ());
        toplevel.set_title(self.title.clone());
        toplevel.set_app_id("evilwm.probe.xdg-window".into());
        base_surface.commit();
        self.xdg_surface = Some((xdg_surface, toplevel));
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for XdgProbeState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            match &interface[..] {
                "wl_compositor" => {
                    let compositor =
                        registry.bind::<wl_compositor::WlCompositor, _, _>(name, 1, qh, ());
                    state.base_surface = Some(compositor.create_surface(qh, ()));
                    if state.wm_base.is_some() {
                        state.init_xdg_surface(qh);
                    }
                }
                "wl_shm" => {
                    let shm = registry.bind::<wl_shm::WlShm, _, _>(name, 1, qh, ());
                    let (w, h) = (320_i32, 220_i32);
                    let mut file = tempfile().expect("temp shm file");
                    draw_probe_buffer(&mut file, w as u32, h as u32).expect("draw probe buffer");
                    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
                    let buffer =
                        pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
                    state.buffer = Some(buffer.clone());
                    if state.configured {
                        let surface = state.base_surface.as_ref().expect("surface");
                        surface.attach(Some(&buffer), 0, 0);
                        surface.commit();
                    }
                }
                "wl_seat" => {
                    registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ());
                }
                "xdg_wm_base" => {
                    state.wm_base =
                        Some(registry.bind::<xdg_wm_base::XdgWmBase, _, _>(name, 1, qh, ()));
                    if state.base_surface.is_some() {
                        state.init_xdg_surface(qh);
                    }
                }
                _ => {}
            }
        }
    }
}

delegate_noop!(XdgProbeState: ignore wl_compositor::WlCompositor);
delegate_noop!(XdgProbeState: ignore wl_surface::WlSurface);
delegate_noop!(XdgProbeState: ignore wl_shm::WlShm);
delegate_noop!(XdgProbeState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(XdgProbeState: ignore wl_buffer::WlBuffer);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for XdgProbeState {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for XdgProbeState {
    fn event(
        state: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            state.configured = true;
            if let (Some(surface), Some(buffer)) = (&state.base_surface, &state.buffer) {
                surface.attach(Some(buffer), 0, 0);
                surface.commit();
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for XdgProbeState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Close = event {
            state.running = false;
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for XdgProbeState {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
            && capabilities.contains(wl_seat::Capability::Keyboard)
        {
            seat.get_keyboard(qh, ());
        }
    }
}

delegate_noop!(XdgProbeState: ignore wl_keyboard::WlKeyboard);

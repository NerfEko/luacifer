use std::{
    fs::File,
    os::unix::io::AsFd,
    thread,
    time::{Duration, Instant},
};

use tempfile::tempfile;
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, delegate_noop,
    protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_registry, wl_seat, wl_shm,
        wl_shm_pool, wl_surface,
    },
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};
use x11rb::{
    COPY_DEPTH_FROM_PARENT, connection::Connection as _, protocol::xproto::*,
    rust_connection::RustConnection, wrapper::ConnectionExt as _,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("evilwm-probe-client: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(mode) = args.next() else {
        print_usage();
        return Ok(());
    };
    if mode == "-h" || mode == "--help" {
        print_usage();
        return Ok(());
    }

    let mut title = String::from("evilwm probe");
    let mut hold_ms = 0_u64;
    let mut namespace = String::from("evilwm-test-panel");

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--title" => {
                title = args.next().ok_or("--title requires a value")?;
            }
            "--hold-ms" => {
                hold_ms = args.next().ok_or("--hold-ms requires a value")?.parse()?;
            }
            "--namespace" => {
                namespace = args.next().ok_or("--namespace requires a value")?;
            }
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    match mode.as_str() {
        "xdg-window" => run_xdg_window(&title, hold_ms)?,
        "layer-panel" => run_layer_panel(&namespace, hold_ms)?,
        "x11-window" => run_x11_window(&title, hold_ms)?,
        _ => return Err(format!("unknown mode: {mode}").into()),
    }

    Ok(())
}

fn print_usage() {
    println!(
        "Usage: evilwm-probe-client <xdg-window|layer-panel|x11-window> [--title TEXT] [--namespace NAME] [--hold-ms N]"
    );
}

fn run_xdg_window(title: &str, hold_ms: u64) -> Result<(), Box<dyn std::error::Error>> {
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
        if let wl_registry::Event::Global { name, interface, .. } = event {
            match &interface[..] {
                "wl_compositor" => {
                    let compositor = registry.bind::<wl_compositor::WlCompositor, _, _>(name, 1, qh, ());
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
                    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
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
                    state.wm_base = Some(registry.bind::<xdg_wm_base::XdgWmBase, _, _>(name, 1, qh, ()));
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

fn run_layer_panel(namespace: &str, hold_ms: u64) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = LayerPanelProbeState {
        namespace: namespace.to_string(),
        output: None,
        compositor: None,
        layer_shell: None,
        surface: None,
        layer_surface: None,
        configured: false,
    };

    let deadline = deadline_from_hold(hold_ms);
    while !deadline_reached(deadline) {
        event_queue.blocking_dispatch(&mut state)?;
    }
    Ok(())
}

struct LayerPanelProbeState {
    namespace: String,
    output: Option<wl_output::WlOutput>,
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    surface: Option<wl_surface::WlSurface>,
    layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    configured: bool,
}

impl LayerPanelProbeState {
    fn maybe_init_layer_surface(&mut self, qh: &QueueHandle<Self>) {
        if self.surface.is_some() {
            return;
        }
        let (Some(compositor), Some(output), Some(layer_shell)) = (
            self.compositor.as_ref(),
            self.output.as_ref(),
            self.layer_shell.as_ref(),
        ) else {
            return;
        };
        let surface = compositor.create_surface(qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            Some(output),
            zwlr_layer_shell_v1::Layer::Top,
            self.namespace.clone(),
            qh,
            (),
        );
        layer_surface.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right,
        );
        layer_surface.set_exclusive_zone(28);
        layer_surface.set_size(0, 28);
        surface.commit();
        self.surface = Some(surface);
        self.layer_surface = Some(layer_surface);
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for LayerPanelProbeState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, .. } = event {
            match &interface[..] {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind::<wl_compositor::WlCompositor, _, _>(name, 1, qh, ()));
                    state.maybe_init_layer_surface(qh);
                }
                "wl_output" => {
                    state.output = Some(registry.bind::<wl_output::WlOutput, _, _>(name, 3, qh, ()));
                    state.maybe_init_layer_surface(qh);
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(name, 4, qh, ()));
                    state.maybe_init_layer_surface(qh);
                }
                _ => {}
            }
        }
    }
}

delegate_noop!(LayerPanelProbeState: ignore wl_compositor::WlCompositor);
delegate_noop!(LayerPanelProbeState: ignore wl_output::WlOutput);
delegate_noop!(LayerPanelProbeState: ignore wl_surface::WlSurface);

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for LayerPanelProbeState {
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

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for LayerPanelProbeState {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, width, height } = event {
            layer_surface.ack_configure(serial);
            if let Some(surface) = state.surface.as_ref() {
                surface.set_buffer_scale(1);
                surface.damage_buffer(0, 0, width as i32, height as i32);
                surface.commit();
            }
            state.configured = true;
        }
    }
}

fn run_x11_window(title: &str, hold_ms: u64) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let win = conn.generate_id()?;
    let aux = CreateWindowAux::new()
        .background_pixel(screen.white_pixel)
        .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY);
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        120,
        120,
        640,
        360,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &aux,
    )?;
    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )?;
    conn.map_window(win)?;
    conn.flush()?;

    if hold_ms == 0 {
        loop {
            if let Some(event) = conn.poll_for_event()? {
                if let x11rb::protocol::Event::DestroyNotify(_) = event {
                    break;
                }
            } else {
                thread::sleep(Duration::from_millis(100));
            }
        }
    } else {
        thread::sleep(Duration::from_millis(hold_ms));
    }

    let _ = conn.destroy_window(win);
    let _ = conn.flush();
    Ok(())
}

fn draw_probe_buffer(file: &mut File, width: u32, height: u32) -> std::io::Result<()> {
    use std::io::Write;
    let mut writer = std::io::BufWriter::new(file);
    for y in 0..height {
        for x in 0..width {
            let a = 0xFF_u8;
            let r = ((width - x) * 0xFF / width) as u8;
            let g = ((x + y) * 0xA0 / (width + height).max(1)) as u8;
            let b = ((height - y) * 0xFF / height) as u8;
            writer.write_all(&[b, g, r, a])?;
        }
    }
    writer.flush()
}

fn deadline_from_hold(hold_ms: u64) -> Option<Instant> {
    if hold_ms == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(hold_ms))
    }
}

fn deadline_reached(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() >= deadline)
}

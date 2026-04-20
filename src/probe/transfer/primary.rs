use std::{
    os::fd::AsFd,
    time::{Duration, Instant},
};

use super::common::{
    check_mime_available, create_payload_pipe, create_single_pixel_buffer, read_from_pipe,
    write_payload_to_fd,
};

use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, delegate_noop, event_created_child,
    protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool,
        wl_surface,
    },
};
use wayland_protocols::{
    wp::primary_selection::zv1::client::{
        zwp_primary_selection_device_manager_v1, zwp_primary_selection_device_v1,
        zwp_primary_selection_offer_v1, zwp_primary_selection_source_v1,
    },
    xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base},
};

/// Result of a primary selection source operation.
#[derive(Debug)]
pub struct PrimarySourceResult {
    pub offered_mimes: Vec<String>,
    pub serial_used: Option<u32>,
    pub selection_set: bool,
    pub send_count: u32,
    pub bytes_written: usize,
    pub error: Option<String>,
}

/// Result of a primary selection sink operation.
#[derive(Debug)]
pub struct PrimarySinkResult {
    pub received_mimes: Vec<String>,
    pub offer_received: bool,
    pub receive_requested: bool,
    pub payload_read_finished: bool,
    pub chosen_mime: Option<String>,
    pub payload: Option<Vec<u8>>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Primary Selection Source
// ---------------------------------------------------------------------------

struct PrimarySourceState {
    payload: Vec<u8>,
    mime: String,
    compositor: Option<wl_compositor::WlCompositor>,
    seat: Option<wl_seat::WlSeat>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    shm: Option<wl_shm::WlShm>,
    surface: Option<wl_surface::WlSurface>,
    xdg_surface: Option<xdg_surface::XdgSurface>,
    toplevel: Option<xdg_toplevel::XdgToplevel>,
    buffer: Option<wl_buffer::WlBuffer>,
    ps_manager: Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
    ps_device: Option<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1>,
    ps_source: Option<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1>,
    configured: bool,
    keyboard_serial: Option<u32>,
    selection_set: bool,
    send_count: u32,
    bytes_written: usize,
    error: Option<String>,
}

pub fn run_primary_source(payload: &[u8], mime: &str, timeout: Duration) -> PrimarySourceResult {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            return PrimarySourceResult {
                offered_mimes: vec![],
                serial_used: None,
                selection_set: false,
                send_count: 0,
                bytes_written: 0,
                error: Some(format!("connect: {e}")),
            };
        }
    };
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = PrimarySourceState {
        payload: payload.to_vec(),
        mime: mime.to_string(),
        compositor: None,
        seat: None,
        wm_base: None,
        shm: None,
        surface: None,
        xdg_surface: None,
        toplevel: None,
        buffer: None,
        ps_manager: None,
        ps_device: None,
        ps_source: None,
        configured: false,
        keyboard_serial: None,
        selection_set: false,
        send_count: 0,
        bytes_written: 0,
        error: None,
    };

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Err(e) = event_queue.blocking_dispatch(&mut state) {
            state.error = Some(format!("dispatch: {e}"));
            break;
        }
        if state.selection_set && state.send_count > 0 {
            std::thread::sleep(Duration::from_millis(100));
            break;
        }
    }

    if let Some(toplevel) = state.toplevel.take() {
        toplevel.destroy();
    }
    if let Some(xdg) = state.xdg_surface.take() {
        xdg.destroy();
    }
    if let Some(surface) = state.surface.take() {
        surface.destroy();
    }
    if let Some(source) = state.ps_source.take() {
        source.destroy();
    }
    let _ = conn.flush();

    PrimarySourceResult {
        offered_mimes: vec![state.mime.clone()],
        serial_used: state.keyboard_serial,
        selection_set: state.selection_set,
        send_count: state.send_count,
        bytes_written: state.bytes_written,
        error: state.error,
    }
}

impl PrimarySourceState {
    fn try_init_surface(&mut self, qh: &QueueHandle<Self>) {
        if self.surface.is_some() {
            return;
        }
        let (Some(compositor), Some(wm_base)) = (self.compositor.as_ref(), self.wm_base.as_ref())
        else {
            return;
        };
        let surface = compositor.create_surface(qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, qh, ());
        let toplevel = xdg_surface.get_toplevel(qh, ());
        toplevel.set_title("evilwm-primary-source".into());
        toplevel.set_app_id("evilwm.probe.primary-source".into());
        surface.commit();
        self.surface = Some(surface);
        self.xdg_surface = Some(xdg_surface);
        self.toplevel = Some(toplevel);
    }

    fn try_set_selection(&mut self, qh: &QueueHandle<Self>) {
        if self.selection_set {
            return;
        }
        let Some(serial) = self.keyboard_serial else {
            return;
        };
        let (Some(manager), Some(device)) = (self.ps_manager.as_ref(), self.ps_device.as_ref())
        else {
            return;
        };
        let source = manager.create_source(qh, ());
        source.offer(self.mime.clone());
        device.set_selection(Some(&source), serial);
        self.ps_source = Some(source);
        self.selection_set = true;
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for PrimarySourceState {
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
                    state.compositor =
                        Some(registry.bind::<wl_compositor::WlCompositor, _, _>(name, 1, qh, ()));
                    state.try_init_surface(qh);
                }
                "xdg_wm_base" => {
                    state.wm_base =
                        Some(registry.bind::<xdg_wm_base::XdgWmBase, _, _>(name, 1, qh, ()));
                    state.try_init_surface(qh);
                }
                "wl_shm" => {
                    let shm = registry.bind::<wl_shm::WlShm, _, _>(name, 1, qh, ());
                    let buffer = create_single_pixel_buffer(&shm, qh).expect("create shm buffer");
                    state.buffer = Some(buffer);
                    state.shm = Some(shm);
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ()));
                }
                "zwp_primary_selection_device_manager_v1" => {
                    let manager = registry.bind::<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1, _, _>(name, 1, qh, ());
                    if let Some(seat) = state.seat.as_ref() {
                        state.ps_device = Some(manager.get_device(seat, qh, ()));
                    }
                    state.ps_manager = Some(manager);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for PrimarySourceState {
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
            if caps.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
            }
            if state.ps_device.is_none()
                && let Some(manager) = state.ps_manager.as_ref()
            {
                state.ps_device = Some(manager.get_device(seat, qh, ()));
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for PrimarySourceState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Enter { serial, .. } = event {
            state.keyboard_serial = Some(serial);
            state.try_set_selection(qh);
        }
        if let wl_keyboard::Event::Key { serial, .. } = event {
            state.keyboard_serial = Some(serial);
            state.try_set_selection(qh);
        }
    }
}

impl Dispatch<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, ()>
    for PrimarySourceState
{
    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        event: zwp_primary_selection_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_primary_selection_source_v1::Event::Send { mime_type, fd } => {
                if mime_type == state.mime {
                    match write_payload_to_fd(fd, &state.payload) {
                        Ok(n) => {
                            state.send_count += 1;
                            state.bytes_written += n;
                        }
                        Err(e) => state.error = Some(e),
                    }
                }
            }
            zwp_primary_selection_source_v1::Event::Cancelled => {
                state.error = Some("source cancelled".into());
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for PrimarySourceState {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for PrimarySourceState {
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
            if let (Some(surface), Some(buffer)) = (&state.surface, &state.buffer) {
                surface.attach(Some(buffer), 0, 0);
                surface.commit();
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for PrimarySourceState {
    fn event(
        _: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        _: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(PrimarySourceState: ignore wl_compositor::WlCompositor);
delegate_noop!(PrimarySourceState: ignore wl_surface::WlSurface);
delegate_noop!(PrimarySourceState: ignore wl_shm::WlShm);
delegate_noop!(PrimarySourceState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(PrimarySourceState: ignore wl_buffer::WlBuffer);
delegate_noop!(PrimarySourceState: ignore zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1);
delegate_noop!(PrimarySourceState: ignore zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1);
delegate_noop!(PrimarySourceState: ignore zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1);

// ---------------------------------------------------------------------------
// Primary Selection Sink
// ---------------------------------------------------------------------------

struct PrimarySinkState {
    desired_mime: String,
    compositor: Option<wl_compositor::WlCompositor>,
    seat: Option<wl_seat::WlSeat>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    shm: Option<wl_shm::WlShm>,
    surface: Option<wl_surface::WlSurface>,
    xdg_surface: Option<xdg_surface::XdgSurface>,
    toplevel: Option<xdg_toplevel::XdgToplevel>,
    buffer: Option<wl_buffer::WlBuffer>,
    ps_manager: Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
    ps_device: Option<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1>,
    configured: bool,
    received_mimes: Vec<String>,
    offer_received: bool,
    receive_requested: bool,
    payload_read_finished: bool,
    chosen_mime: Option<String>,
    payload: Option<Vec<u8>>,
    done: bool,
    error: Option<String>,
    pending_offer: Option<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1>,
}

pub fn run_primary_sink(desired_mime: &str, timeout: Duration) -> PrimarySinkResult {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            return PrimarySinkResult {
                received_mimes: vec![],
                offer_received: false,
                receive_requested: false,
                payload_read_finished: false,
                chosen_mime: None,
                payload: None,
                error: Some(format!("connect: {e}")),
            };
        }
    };
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = PrimarySinkState {
        desired_mime: desired_mime.to_string(),
        compositor: None,
        seat: None,
        wm_base: None,
        shm: None,
        surface: None,
        xdg_surface: None,
        toplevel: None,
        buffer: None,
        ps_manager: None,
        ps_device: None,
        configured: false,
        received_mimes: vec![],
        offer_received: false,
        receive_requested: false,
        payload_read_finished: false,
        chosen_mime: None,
        payload: None,
        done: false,
        error: None,
        pending_offer: None,
    };

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline && !state.done {
        if let Err(e) = event_queue.blocking_dispatch(&mut state) {
            state.error = Some(format!("dispatch: {e}"));
            break;
        }
    }

    if let Some(toplevel) = state.toplevel.take() {
        toplevel.destroy();
    }
    if let Some(xdg) = state.xdg_surface.take() {
        xdg.destroy();
    }
    if let Some(surface) = state.surface.take() {
        surface.destroy();
    }
    let _ = conn.flush();

    PrimarySinkResult {
        received_mimes: state.received_mimes,
        offer_received: state.offer_received,
        receive_requested: state.receive_requested,
        payload_read_finished: state.payload_read_finished,
        chosen_mime: state.chosen_mime,
        payload: state.payload,
        error: state.error,
    }
}

impl PrimarySinkState {
    fn try_init_surface(&mut self, qh: &QueueHandle<Self>) {
        if self.surface.is_some() {
            return;
        }
        let (Some(compositor), Some(wm_base)) = (self.compositor.as_ref(), self.wm_base.as_ref())
        else {
            return;
        };
        let surface = compositor.create_surface(qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, qh, ());
        let toplevel = xdg_surface.get_toplevel(qh, ());
        toplevel.set_title("evilwm-primary-sink".into());
        toplevel.set_app_id("evilwm.probe.primary-sink".into());
        surface.commit();
        self.surface = Some(surface);
        self.xdg_surface = Some(xdg_surface);
        self.toplevel = Some(toplevel);
    }

    fn try_read_selection(&mut self) {
        let Some(offer) = self.pending_offer.take() else {
            return;
        };
        if let Err(e) = check_mime_available(&self.desired_mime, &self.received_mimes) {
            self.error = Some(e);
            self.done = true;
            return;
        }
        let (read_fd, write_fd) = match create_payload_pipe() {
            Ok(fds) => fds,
            Err(e) => {
                self.error = Some(e);
                self.done = true;
                return;
            }
        };
        offer.receive(self.desired_mime.clone(), write_fd.as_fd());
        self.receive_requested = true;
        drop(write_fd);
        self.chosen_mime = Some(self.desired_mime.clone());
        match read_from_pipe(read_fd) {
            Ok(buf) => {
                self.payload_read_finished = true;
                self.payload = Some(buf);
            }
            Err(e) => self.error = Some(e),
        }
        self.done = true;
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for PrimarySinkState {
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
                    state.compositor =
                        Some(registry.bind::<wl_compositor::WlCompositor, _, _>(name, 1, qh, ()));
                    state.try_init_surface(qh);
                }
                "xdg_wm_base" => {
                    state.wm_base =
                        Some(registry.bind::<xdg_wm_base::XdgWmBase, _, _>(name, 1, qh, ()));
                    state.try_init_surface(qh);
                }
                "wl_shm" => {
                    let shm = registry.bind::<wl_shm::WlShm, _, _>(name, 1, qh, ());
                    let buffer = create_single_pixel_buffer(&shm, qh).expect("create shm buffer");
                    state.buffer = Some(buffer);
                    state.shm = Some(shm);
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ()));
                }
                "zwp_primary_selection_device_manager_v1" => {
                    let manager = registry.bind::<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1, _, _>(name, 1, qh, ());
                    if let Some(seat) = state.seat.as_ref() {
                        state.ps_device = Some(manager.get_device(seat, qh, ()));
                    }
                    state.ps_manager = Some(manager);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for PrimarySinkState {
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
            if caps.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
            }
            if state.ps_device.is_none()
                && let Some(manager) = state.ps_manager.as_ref()
            {
                state.ps_device = Some(manager.get_device(seat, qh, ()));
            }
        }
    }
}

delegate_noop!(PrimarySinkState: ignore wl_keyboard::WlKeyboard);
delegate_noop!(PrimarySinkState: ignore wl_compositor::WlCompositor);
delegate_noop!(PrimarySinkState: ignore wl_surface::WlSurface);
delegate_noop!(PrimarySinkState: ignore wl_shm::WlShm);
delegate_noop!(PrimarySinkState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(PrimarySinkState: ignore wl_buffer::WlBuffer);
delegate_noop!(PrimarySinkState: ignore zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1);

impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, ()>
    for PrimarySinkState
{
    event_created_child!(PrimarySinkState, zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => (zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ())
    ]);

    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_device_v1::Event::Selection { id: Some(offer) } = event {
            state.offer_received = true;
            state.pending_offer = Some(offer);
            state.try_read_selection();
        }
    }
}

impl Dispatch<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()> for PrimarySinkState {
    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
        event: zwp_primary_selection_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = event {
            state.received_mimes.push(mime_type);
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for PrimarySinkState {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for PrimarySinkState {
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
            if let (Some(surface), Some(buffer)) = (&state.surface, &state.buffer) {
                surface.attach(Some(buffer), 0, 0);
                surface.commit();
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for PrimarySinkState {
    fn event(
        _: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        _: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

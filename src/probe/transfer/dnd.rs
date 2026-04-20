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
        wl_buffer, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
        wl_data_source, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
        wl_surface,
    },
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

// ---------------------------------------------------------------------------
// DnD Source
// ---------------------------------------------------------------------------

/// Result of a DnD source probe operation.
pub struct DndSourceResult {
    pub offered_mimes: Vec<String>,
    pub pointer_serial_obtained: bool,
    pub start_drag_attempted: bool,
    pub send_count: u32,
    pub bytes_written: usize,
    /// Why `start_drag` was not called, when `start_drag_attempted` is false.
    pub blocked_reason: Option<String>,
    pub error: Option<String>,
}

struct DndSourceState {
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
    data_manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    data_device: Option<wl_data_device::WlDataDevice>,
    data_source: Option<wl_data_source::WlDataSource>,
    configured: bool,
    pointer: Option<wl_pointer::WlPointer>,
    pointer_serial: Option<u32>,
    start_drag_attempted: bool,
    send_count: u32,
    bytes_written: usize,
    done: bool,
    error: Option<String>,
}

/// Run a DnD source probe that offers `mime` and waits for a pointer button press to
/// call `start_drag`.  In automated test contexts, no pointer press arrives, so the
/// probe times out and reports `start_drag_attempted: false` with
/// `blocked_reason: "no_pointer_button_serial"`.
///
/// For manual testing: run against a live compositor, click on the probe window, and
/// the drag will be initiated.
pub fn run_dnd_source(payload: &[u8], mime: &str, timeout: Duration) -> DndSourceResult {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            return DndSourceResult {
                offered_mimes: vec![],
                pointer_serial_obtained: false,
                start_drag_attempted: false,
                send_count: 0,
                bytes_written: 0,
                blocked_reason: Some("no_pointer_button_serial".into()),
                error: Some(format!("connect: {e}")),
            };
        }
    };
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = DndSourceState {
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
        data_manager: None,
        data_device: None,
        data_source: None,
        configured: false,
        pointer: None,
        pointer_serial: None,
        start_drag_attempted: false,
        send_count: 0,
        bytes_written: 0,
        done: false,
        error: None,
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
    if let Some(source) = state.data_source.take() {
        source.destroy();
    }
    let _ = conn.flush();

    let blocked_reason = if !state.start_drag_attempted {
        Some("no_pointer_button_serial".into())
    } else {
        None
    };

    DndSourceResult {
        offered_mimes: vec![state.mime.clone()],
        pointer_serial_obtained: state.pointer_serial.is_some(),
        start_drag_attempted: state.start_drag_attempted,
        send_count: state.send_count,
        bytes_written: state.bytes_written,
        blocked_reason,
        error: state.error,
    }
}

impl DndSourceState {
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
        toplevel.set_title("evilwm-dnd-source".into());
        toplevel.set_app_id("evilwm.probe.dnd-source".into());
        surface.commit();
        self.surface = Some(surface);
        self.xdg_surface = Some(xdg_surface);
        self.toplevel = Some(toplevel);
    }

    fn try_start_drag(&mut self, qh: &QueueHandle<Self>) {
        if self.start_drag_attempted {
            return;
        }
        let Some(serial) = self.pointer_serial else {
            return;
        };
        let (Some(manager), Some(device), Some(surface)) = (
            self.data_manager.as_ref(),
            self.data_device.as_ref(),
            self.surface.as_ref(),
        ) else {
            return;
        };
        let source = manager.create_data_source(qh, ());
        source.offer(self.mime.clone());
        device.start_drag(
            Some(&source),
            surface,
            None::<&wl_surface::WlSurface>,
            serial,
        );
        self.data_source = Some(source);
        self.start_drag_attempted = true;
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for DndSourceState {
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
                    let seat = registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ());
                    state.seat = Some(seat);
                }
                "wl_data_device_manager" => {
                    let manager = registry
                        .bind::<wl_data_device_manager::WlDataDeviceManager, _, _>(name, 3, qh, ());
                    if let Some(seat) = state.seat.as_ref() {
                        state.data_device = Some(manager.get_data_device(seat, qh, ()));
                    }
                    state.data_manager = Some(manager);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for DndSourceState {
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
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
            if state.data_device.is_none()
                && let Some(manager) = state.data_manager.as_ref()
            {
                state.data_device = Some(manager.get_data_device(seat, qh, ()));
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for DndSourceState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button {
            serial,
            state: WEnum::Value(wl_pointer::ButtonState::Pressed),
            ..
        } = event
        {
            state.pointer_serial = Some(serial);
            state.try_start_drag(qh);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for DndSourceState {
    fn event(
        state: &mut Self,
        _: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
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
            wl_data_source::Event::Cancelled => {
                state.done = true;
            }
            wl_data_source::Event::DndDropPerformed => {
                state.done = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for DndSourceState {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for DndSourceState {
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

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for DndSourceState {
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

delegate_noop!(DndSourceState: ignore wl_compositor::WlCompositor);
delegate_noop!(DndSourceState: ignore wl_surface::WlSurface);
delegate_noop!(DndSourceState: ignore wl_shm::WlShm);
delegate_noop!(DndSourceState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(DndSourceState: ignore wl_buffer::WlBuffer);
delegate_noop!(DndSourceState: ignore wl_data_device::WlDataDevice);
delegate_noop!(DndSourceState: ignore wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(DndSourceState: ignore wl_data_offer::WlDataOffer);

// ---------------------------------------------------------------------------
// DnD Target
// ---------------------------------------------------------------------------

/// Result of a DnD target probe operation.
pub struct DndTargetResult {
    pub enter_received: bool,
    pub offered_mimes: Vec<String>,
    pub offer_received: bool,
    pub receive_requested: bool,
    pub payload_read_finished: bool,
    pub chosen_mime: Option<String>,
    pub payload: Option<Vec<u8>>,
    pub drop_received: bool,
    pub error: Option<String>,
}

struct DndTargetState {
    desired_mime: String,
    compositor: Option<wl_compositor::WlCompositor>,
    seat: Option<wl_seat::WlSeat>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    shm: Option<wl_shm::WlShm>,
    surface: Option<wl_surface::WlSurface>,
    xdg_surface: Option<xdg_surface::XdgSurface>,
    toplevel: Option<xdg_toplevel::XdgToplevel>,
    buffer: Option<wl_buffer::WlBuffer>,
    data_manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    data_device: Option<wl_data_device::WlDataDevice>,
    configured: bool,
    enter_received: bool,
    offered_mimes: Vec<String>,
    offer_received: bool,
    receive_requested: bool,
    payload_read_finished: bool,
    chosen_mime: Option<String>,
    payload: Option<Vec<u8>>,
    drop_received: bool,
    /// Negotiated DnD action received via wl_data_offer::Event::Action before calling finish().
    received_action: Option<wl_data_device_manager::DndAction>,
    done: bool,
    error: Option<String>,
    pending_offer: Option<wl_data_offer::WlDataOffer>,
}

/// Run a DnD target probe that maps a surface and waits for a DnD enter event.
///
/// In automated test contexts, no drag will arrive (the source probe cannot start
/// one without a pointer button serial), so the probe times out and reports
/// `enter_received: false`.
///
/// For manual testing: run alongside a DnD source, drag over the probe window, and
/// drop to prove the payload round-trip.
pub fn run_dnd_target(desired_mime: &str, timeout: Duration) -> DndTargetResult {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            return DndTargetResult {
                enter_received: false,
                offered_mimes: vec![],
                offer_received: false,
                receive_requested: false,
                payload_read_finished: false,
                chosen_mime: None,
                payload: None,
                drop_received: false,
                error: Some(format!("connect: {e}")),
            };
        }
    };
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = DndTargetState {
        desired_mime: desired_mime.to_string(),
        compositor: None,
        seat: None,
        wm_base: None,
        shm: None,
        surface: None,
        xdg_surface: None,
        toplevel: None,
        buffer: None,
        data_manager: None,
        data_device: None,
        configured: false,
        enter_received: false,
        offered_mimes: vec![],
        offer_received: false,
        receive_requested: false,
        payload_read_finished: false,
        chosen_mime: None,
        payload: None,
        drop_received: false,
        received_action: None,
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

    DndTargetResult {
        enter_received: state.enter_received,
        offered_mimes: state.offered_mimes,
        offer_received: state.offer_received,
        receive_requested: state.receive_requested,
        payload_read_finished: state.payload_read_finished,
        chosen_mime: state.chosen_mime,
        payload: state.payload,
        drop_received: state.drop_received,
        error: state.error,
    }
}

impl DndTargetState {
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
        toplevel.set_title("evilwm-dnd-target".into());
        toplevel.set_app_id("evilwm.probe.dnd-target".into());
        surface.commit();
        self.surface = Some(surface);
        self.xdg_surface = Some(xdg_surface);
        self.toplevel = Some(toplevel);
    }

    fn try_receive_drop(&mut self) {
        let Some(offer) = self.pending_offer.take() else {
            return;
        };
        if let Err(e) = check_mime_available(&self.desired_mime, &self.offered_mimes) {
            self.error = Some(e);
            offer.destroy();
            self.done = true;
            return;
        }
        let (read_fd, write_fd) = match create_payload_pipe() {
            Ok(fds) => fds,
            Err(e) => {
                self.error = Some(e);
                offer.destroy();
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
        // finish() is only valid after the negotiated action is confirmed via
        // wl_data_offer::Event::Action; guard it to avoid a protocol error.
        if self.received_action.is_some() {
            offer.finish();
        }
        offer.destroy();
        self.done = true;
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for DndTargetState {
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
                    let seat = registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ());
                    state.seat = Some(seat);
                }
                "wl_data_device_manager" => {
                    let manager = registry
                        .bind::<wl_data_device_manager::WlDataDeviceManager, _, _>(name, 3, qh, ());
                    if let Some(seat) = state.seat.as_ref() {
                        state.data_device = Some(manager.get_data_device(seat, qh, ()));
                    }
                    state.data_manager = Some(manager);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for DndTargetState {
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
            if state.data_device.is_none()
                && let Some(manager) = state.data_manager.as_ref()
            {
                state.data_device = Some(manager.get_data_device(seat, qh, ()));
            }
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for DndTargetState {
    event_created_child!(DndTargetState, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ())
    ]);

    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::Enter {
                id: Some(offer), ..
            } => {
                state.enter_received = true;
                state.offer_received = true;
                state.received_action = None;
                // serial in accept() is a client-generated counter (not the enter serial);
                // 0 is conventional for v3 where finish() is the authoritative signal.
                offer.accept(0, Some(state.desired_mime.clone()));
                offer.set_actions(
                    wl_data_device_manager::DndAction::Copy,
                    wl_data_device_manager::DndAction::Copy,
                );
                state.pending_offer = Some(offer);
            }
            wl_data_device::Event::Drop => {
                state.drop_received = true;
                state.try_receive_drop();
            }
            wl_data_device::Event::Leave => {
                // The spec requires the client to destroy the offer on leave.
                if let Some(offer) = state.pending_offer.take() {
                    offer.destroy();
                }
                state.received_action = None;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for DndTargetState {
    fn event(
        state: &mut Self,
        _: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_offer::Event::Offer { mime_type } => {
                state.offered_mimes.push(mime_type);
            }
            wl_data_offer::Event::Action {
                dnd_action: WEnum::Value(action),
            } => {
                // Required before calling finish(); guards the finish() call in try_receive_drop.
                state.received_action = Some(action);
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for DndTargetState {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for DndTargetState {
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

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for DndTargetState {
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

delegate_noop!(DndTargetState: ignore wl_compositor::WlCompositor);
delegate_noop!(DndTargetState: ignore wl_surface::WlSurface);
delegate_noop!(DndTargetState: ignore wl_shm::WlShm);
delegate_noop!(DndTargetState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(DndTargetState: ignore wl_buffer::WlBuffer);
delegate_noop!(DndTargetState: ignore wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(DndTargetState: ignore wl_keyboard::WlKeyboard);

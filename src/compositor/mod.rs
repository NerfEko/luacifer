mod event_loop;
mod hooks;
mod input;
mod ipc;
mod output_runtime;
mod protocols;
mod rendering;
#[cfg(feature = "udev")]
mod udev;
mod window_management;
#[cfg(feature = "xwayland")]
mod xwayland;

pub use crate::headless::{HeadlessOptions, HeadlessReport, HeadlessSession, run_headless};
pub use event_loop::run_winit;
#[cfg(feature = "udev")]
pub use udev::run_udev;

#[cfg(test)]
use event_loop::build_spawn_command;
#[cfg(feature = "udev")]
use event_loop::publish_wayland_display;
use event_loop::spawn_client;
#[cfg(any(test, feature = "udev"))]
use event_loop::startup_commands;
#[cfg(feature = "lua")]
use hooks::format_live_hook_error;
#[cfg(test)]
use input::apply_trackpad_swipe_to_viewport;
#[cfg(test)]
use input::pinch_relative_factor;
#[cfg(test)]
use ipc::validate_ipc_screenshot_path;
#[cfg(feature = "xwayland")]
use protocols::ClientState;
use protocols::{window_matches_surface, window_toplevel};
#[cfg(feature = "udev")]
use rendering::{build_cursor_elements, build_live_space_elements};
use rendering::{
    build_winit_space_elements, format_keyspec, lock_overlay_elements, render_stack_front_to_back,
    resize_edges_from, solid_elements_from_draw_commands, write_ppm_screenshot,
};
#[cfg(feature = "xwayland")]
use xwayland::spawn_xwayland;

#[cfg(feature = "udev")]
use std::{cell::RefCell, rc::Rc};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    error::Error,
    ffi::OsString,
    io,
    os::unix::io::OwnedFd,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, GestureBeginEvent,
            GesturePinchUpdateEvent, GestureSwipeUpdateEvent, InputBackend, InputEvent, KeyState,
            KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
        },
        renderer::{
            ExportMem, TextureMapping, damage::OutputDamageTracker, gles::GlesRenderer,
            utils::on_commit_buffer_handler,
        },
        winit::{self, WinitEvent},
    },
    desktop::{
        LayerSurface as DesktopLayerSurface, PopupKind, PopupManager, Space, Window,
        WindowSurfaceType, find_popup_root_surface, get_popup_toplevel_coords,
        layer_map_for_output,
    },
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, ModifiersState, xkb},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Client, Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer, wl_output::WlOutput, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point as SmithayPoint, Rectangle, SERIAL_COUNTER, Serial, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface, with_states,
        },
        idle_inhibit::{IdleInhibitHandler, IdleInhibitManagerState},
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
            primary_selection::{
                PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
            },
        },
        shell::{
            wlr_layer::{
                Layer as WlrLayer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler,
                WlrLayerShellState,
            },
            xdg::{
                PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
                XdgToplevelSurfaceData, decoration::XdgDecorationState,
            },
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
    },
};

use crate::{
    canvas::{Point, Rect, Size, Viewport},
    input::{BindingMap, ModifierSet},
    lua::{
        ActionTarget, Config, ConfigError, OutputSnapshot, RuntimeStateSnapshot, ViewportSnapshot,
        WindowSnapshot,
    },
    output::OutputState,
    output_management_protocol::{
        OutputManagementHandler as OutputProtocolHandler,
        OutputManagementState as OutputProtocolState,
    },
    window::{
        AppliedWindowRules, FocusStack, PlacementPolicy, ResizeEdges, Window as WindowModel,
        WindowId, WindowProperties, WindowRule,
    },
};

use crate::lua::{DrawCommand, DrawLayer, DrawSpace};
#[cfg(feature = "lua")]
use crate::lua::{LiveLuaHooks, ResolveFocusRequest};
#[cfg(feature = "udev")]
use smithay::backend::renderer::element::utils::RescaleRenderElement;
#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
use smithay::{
    backend::renderer::{
        ImportAll,
        element::{Id, Kind, solid::SolidColorRenderElement},
    },
    utils::Physical,
};
#[cfg(feature = "xwayland")]
use smithay::{
    wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandClientData, XWaylandEvent,
        xwm::{X11Window, XwmHandler, XwmId},
    },
};

#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub command: Option<String>,
    pub config_path: Option<std::path::PathBuf>,
    pub config: Option<Config>,
}

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
smithay::backend::renderer::element::render_elements! {
    LiveRenderElements<R, E> where R: ImportAll;
    Space=smithay::desktop::space::SpaceRenderElements<R, E>,
    Custom=SolidColorRenderElement,
}

#[cfg(feature = "udev")]
type UdevRenderElements<R, E> = LiveRenderElements<R, E>;

#[cfg(feature = "udev")]
type TtyControlCallback = dyn FnMut(TtyControlAction);

#[cfg(feature = "udev")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TtyControlAction {
    Quit,
    SwitchVt(i32),
}

#[cfg(feature = "udev")]
fn tty_control_action_for(
    key: &str,
    modifiers: ModifierSet,
    tty_config: &crate::lua::TtyConfig,
) -> Option<TtyControlAction> {
    let quit_modifiers = ModifierSet::from_names(&tty_config.quit_mods);
    if modifiers == quit_modifiers && key.eq_ignore_ascii_case(&tty_config.quit_key) {
        return Some(TtyControlAction::Quit);
    }

    let vt_switch_modifiers = ModifierSet::from_names(&tty_config.vt_switch_modifiers);
    if modifiers != vt_switch_modifiers {
        return None;
    }

    let vt_suffix = key.strip_prefix('F').or_else(|| key.strip_prefix('f'))?;
    let Ok(vt) = vt_suffix.parse::<i32>() else {
        return None;
    };
    if (1..=12).contains(&vt) {
        Some(TtyControlAction::SwitchVt(vt))
    } else {
        None
    }
}

pub struct EvilWm {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub ipc_socket_path: std::path::PathBuf,
    pub display_handle: DisplayHandle,
    pub space: Space<Window>,
    pub loop_signal: LoopSignal,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub layer_shell_state: WlrLayerShellState,
    #[cfg(feature = "xwayland")]
    pub xwayland_shell_state: XWaylandShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub output_management_protocol_state: OutputProtocolState,
    pub seat_state: SeatState<EvilWm>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub idle_inhibit_state: IdleInhibitManagerState,
    pub popups: PopupManager,
    pub seat: Seat<Self>,
    pub output_state: OutputState,
    pub output_states: BTreeMap<String, OutputState>,
    pub focus_stack: FocusStack,
    pub fallback_placement_policy: PlacementPolicy,
    pub bindings: BindingMap,
    pub window_rules: Vec<WindowRule>,
    pub next_window_id: u64,
    pub window_models: BTreeMap<WindowId, WindowModel>,
    pub surface_window_ids: HashMap<WlSurface, WindowId>,
    pub window_surfaces: HashMap<WindowId, WlSurface>,
    remembered_app_sizes: HashMap<String, Size>,
    pending_client_default_size: HashSet<WindowId>,
    pub config: Option<Config>,
    pub config_path: Option<std::path::PathBuf>,
    redraw_requested: bool,
    trackpad_pinch_scale: Option<f64>,
    #[cfg(feature = "udev")]
    tty_control: Option<Rc<RefCell<Box<TtyControlCallback>>>>,
    #[cfg(feature = "udev")]
    tty_no_scanout_warned: bool,
    #[cfg(feature = "udev")]
    tty_session_active: bool,
    #[cfg(feature = "lua")]
    pub live_lua: Option<LiveLuaHooks>,
    #[cfg(feature = "lua")]
    live_hook_errors: BTreeMap<String, LiveHookErrorState>,
    active_interactive_op: Option<ActiveInteractiveOp>,
    last_pointer_button_pressed: Option<u32>,
    suppress_pointer_button_release: Option<u32>,
    warned_missing_move_update_hook: bool,
    warned_missing_resize_update_hook: bool,
    missing_client_compositor_state_warned: AtomicBool,
    session_locked: bool,
    idle_inhibitors: HashSet<WlSurface>,
    pending_screenshot_path: Option<std::path::PathBuf>,
    event_log_path: Option<std::path::PathBuf>,
    ipc_trace_dir: Option<std::path::PathBuf>,
    event_sequence: u64,
    #[cfg(feature = "xwayland")]
    x11_wm: Option<X11Wm>,
    #[cfg(feature = "xwayland")]
    pending_x11_windows: HashSet<X11Window>,
}

impl Drop for EvilWm {
    fn drop(&mut self) {
        self.emit_event(
            "shutdown",
            serde_json::json!({
                "backend": if cfg!(feature = "udev") && self.is_tty_backend() {
                    "udev"
                } else {
                    "winit"
                },
                "socket": self.socket_name.to_string_lossy(),
            }),
        );
        if self.ipc_socket_path.exists() {
            let _ = std::fs::remove_file(&self.ipc_socket_path);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveInteractiveKind {
    Move,
    Resize,
    PanCanvas,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveInteractiveOp {
    kind: ActiveInteractiveKind,
    window_id: WindowId,
    last_pointer: Point,
    total_delta: crate::canvas::Vec2,
    resize_edges: Option<ResizeEdges>,
    initiating_button: Option<u32>,
}

impl ActiveInteractiveOp {
    fn new(
        kind: ActiveInteractiveKind,
        window_id: WindowId,
        pointer: Point,
        resize_edges: Option<ResizeEdges>,
        initiating_button: Option<u32>,
    ) -> Self {
        Self {
            kind,
            window_id,
            last_pointer: pointer,
            total_delta: crate::canvas::Vec2::new(0.0, 0.0),
            resize_edges,
            initiating_button,
        }
    }

    fn advance(&mut self, pointer: Point) -> crate::canvas::Vec2 {
        let delta = pointer - self.last_pointer;
        self.last_pointer = pointer;
        self.total_delta = self.total_delta + delta;
        delta
    }

    fn total_delta(&self) -> crate::canvas::Vec2 {
        self.total_delta
    }

    fn resize_edges(&self) -> Option<ResizeEdges> {
        self.resize_edges
    }

    fn should_end_on_button(&self, button: u32) -> bool {
        self.initiating_button
            .is_none_or(|initiating| initiating == button)
    }
}

#[cfg(feature = "lua")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveHookErrorState {
    count: u64,
    last_error: String,
}

fn initialize_jsonl_file(path: &std::path::Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::File::create(path)?;
    Ok(())
}

fn append_jsonl(path: &std::path::Path, value: &serde_json::Value) -> io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn initialize_ipc_trace_dir(dir: &std::path::Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    initialize_jsonl_file(&dir.join("requests.jsonl"))?;
    initialize_jsonl_file(&dir.join("responses.jsonl"))?;
    Ok(())
}

fn compile_window_rules(config: Option<&Config>) -> Vec<WindowRule> {
    config
        .map(|cfg| {
            cfg.rules
                .iter()
                .map(|rule| WindowRule {
                    app_id: rule.app_id.clone(),
                    title_contains: rule.title_contains.clone(),
                    floating: rule.floating,
                    exclude_from_focus: rule.exclude_from_focus,
                    default_size: match (rule.width, rule.height) {
                        (Some(w), Some(h)) => Some(Size::new(w, h)),
                        _ => None,
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

fn window_properties_from_toplevel(surface: &ToplevelSurface) -> WindowProperties {
    with_states(surface.wl_surface(), |states| {
        let attributes = states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .unwrap()
            .lock()
            .unwrap();

        WindowProperties {
            app_id: attributes.app_id.clone(),
            title: attributes.title.clone(),
            pid: None,
        }
    })
}

impl EvilWm {
    pub fn new(
        event_loop: &mut EventLoop<Self>,
        display: Display<Self>,
        config_path: Option<std::path::PathBuf>,
        config: Option<Config>,
    ) -> Result<Self, Box<dyn Error>> {
        let start_time = std::time::Instant::now();
        let dh = display.handle();
        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        #[cfg(feature = "xwayland")]
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let popups = PopupManager::default();
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let output_management_protocol_state = OutputProtocolState::new::<Self, _>(&dh, |_| true);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let idle_inhibit_state = IdleInhibitManagerState::new::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "evilwm");
        seat.add_keyboard(Default::default(), 200, 25)
            .map_err(|error| io::Error::other(format!("failed to add keyboard seat: {error}")))?;
        seat.add_pointer();
        let space = Space::default();
        let socket_name = Self::init_wayland_listener(display, event_loop)?;
        let ipc_socket_path = Self::init_ipc_listener(event_loop)?;
        let loop_signal = event_loop.get_signal();

        let screen_size = Size::new(1280.0, 720.0);
        let mut viewport = Viewport::new(screen_size);
        let mut fallback_placement_policy = PlacementPolicy::default();
        if let Some(cfg) = &config {
            viewport = viewport
                .try_with_zoom_limits(cfg.canvas.min_zoom, cfg.canvas.max_zoom)
                .map_err(|error| {
                    io::Error::other(format!("invalid canvas zoom limits: {error}"))
                })?;
            fallback_placement_policy.default_size =
                Size::new(cfg.placement.default_size.0, cfg.placement.default_size.1);
            fallback_placement_policy.padding = cfg.placement.padding;
            fallback_placement_policy.cascade_step = crate::canvas::Vec2::new(
                cfg.placement.cascade_step.0,
                cfg.placement.cascade_step.1,
            );
        }

        let bindings = config
            .as_ref()
            .map(|cfg| {
                BindingMap::from_config(&cfg.bindings, cfg.canvas.pan_step, cfg.canvas.zoom_step)
            })
            .unwrap_or_default();
        let window_rules = compile_window_rules(config.as_ref());

        let event_log_path = std::env::var_os("EVILWM_EVENT_LOG").map(std::path::PathBuf::from);
        if let Some(path) = event_log_path.as_deref() {
            initialize_jsonl_file(path)?;
        }
        let ipc_trace_dir = std::env::var_os("EVILWM_IPC_TRACE_DIR").map(std::path::PathBuf::from);
        if let Some(dir) = ipc_trace_dir.as_deref() {
            initialize_ipc_trace_dir(dir)?;
        }

        let output_state = OutputState::with_viewport("winit", Point::new(0.0, 0.0), viewport);

        #[cfg(feature = "lua")]
        let live_lua = config_path
            .as_deref()
            .and_then(|path| {
                path.parent()
                    .map(std::path::Path::to_path_buf)
                    .map(|base| (path, base))
            })
            .map(|(path, base)| {
                let hooks = LiveLuaHooks::new(base)?;
                hooks.load_script_file(path)?;
                Ok::<LiveLuaHooks, crate::lua::ConfigError>(hooks)
            })
            .transpose()
            .map_err(|error| {
                io::Error::other(format!("failed to initialize live lua hooks: {error}"))
            })?;

        Ok(Self {
            start_time,
            socket_name,
            ipc_socket_path,
            display_handle: dh,
            space,
            loop_signal,
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            layer_shell_state,
            #[cfg(feature = "xwayland")]
            xwayland_shell_state,
            shm_state,
            output_manager_state,
            output_management_protocol_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            idle_inhibit_state,
            popups,
            seat,
            output_state,
            output_states: BTreeMap::new(),
            focus_stack: FocusStack::default(),
            fallback_placement_policy,
            bindings,
            window_rules,
            next_window_id: 1,
            window_models: BTreeMap::new(),
            surface_window_ids: HashMap::new(),
            window_surfaces: HashMap::new(),
            remembered_app_sizes: HashMap::new(),
            pending_client_default_size: HashSet::new(),
            config,
            config_path,
            redraw_requested: true,
            trackpad_pinch_scale: None,
            #[cfg(feature = "udev")]
            tty_control: None,
            #[cfg(feature = "udev")]
            tty_no_scanout_warned: false,
            #[cfg(feature = "udev")]
            tty_session_active: true,
            #[cfg(feature = "lua")]
            live_lua,
            #[cfg(feature = "lua")]
            live_hook_errors: BTreeMap::new(),
            active_interactive_op: None,
            last_pointer_button_pressed: None,
            suppress_pointer_button_release: None,
            warned_missing_move_update_hook: false,
            warned_missing_resize_update_hook: false,
            missing_client_compositor_state_warned: AtomicBool::new(false),
            session_locked: false,
            idle_inhibitors: HashSet::new(),
            pending_screenshot_path: None,
            event_log_path,
            ipc_trace_dir,
            event_sequence: 0,
            #[cfg(feature = "xwayland")]
            x11_wm: None,
            #[cfg(feature = "xwayland")]
            pending_x11_windows: HashSet::new(),
        })
    }

    pub fn request_redraw(&mut self) {
        self.redraw_requested = true;
    }

    pub fn redraw_requested(&self) -> bool {
        self.redraw_requested
    }

    pub fn clear_redraw_request(&mut self) {
        self.redraw_requested = false;
    }

    pub(crate) fn emit_event(&mut self, kind: &str, data: serde_json::Value) {
        let Some(path) = self.event_log_path.clone() else {
            return;
        };

        self.event_sequence = self.event_sequence.saturating_add(1);
        let entry = serde_json::json!({
            "seq": self.event_sequence,
            "elapsed_ms": self.start_time.elapsed().as_millis() as u64,
            "kind": kind,
            "data": data,
        });
        if let Err(error) = append_jsonl(&path, &entry) {
            eprintln!("failed to append event log {}: {error}", path.display());
        }
    }

    pub(crate) fn trace_ipc_json(&mut self, file_name: &str, payload: serde_json::Value) {
        let Some(dir) = self.ipc_trace_dir.clone() else {
            return;
        };
        let path = dir.join(file_name);
        if let Err(error) = append_jsonl(&path, &payload) {
            eprintln!("failed to append ipc trace {}: {error}", path.display());
        }
    }

    fn init_wayland_listener(
        display: Display<EvilWm>,
        event_loop: &mut EventLoop<Self>,
    ) -> Result<OsString, Box<dyn Error>> {
        let listening_socket = ListeningSocketSource::new_auto().map_err(|error| {
            io::Error::other(format!("failed to create wayland socket: {error}"))
        })?;
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                if let Err(error) = state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                {
                    eprintln!("failed to insert wayland client: {error}");
                }
            })
            .map_err(|error| {
                io::Error::other(format!("failed to init wayland listener: {error}"))
            })?;

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // SAFETY: calloop dispatches this source on the compositor thread with
                    // exclusive mutable access to the display during the callback.
                    let result = unsafe { display.get_mut().dispatch_clients(state) };
                    if let Err(error) = result {
                        eprintln!("wayland client dispatch error: {error}");
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to register wayland dispatch source: {error}"
                ))
            })?;

        Ok(socket_name)
    }

    pub fn surface_under(
        &self,
        pos: SmithayPoint<f64, Logical>,
    ) -> Option<(WlSurface, SmithayPoint<f64, Logical>)> {
        if !self.is_tty_backend()
            && let Some(output) = self.output_at_screen_position(pos)
            && let Some(output_geo) = self.space.output_geometry(&output)
        {
            let local = pos - output_geo.loc.to_f64();
            let map = layer_map_for_output(&output);
            for layer in [
                WlrLayer::Overlay,
                WlrLayer::Top,
                WlrLayer::Bottom,
                WlrLayer::Background,
            ] {
                if let Some(layer_surface) = map.layer_under(layer, local)
                    && let Some(layer_geo) = map.layer_geometry(layer_surface)
                    && let Some((surface, point)) = layer_surface
                        .surface_under(local - layer_geo.loc.to_f64(), WindowSurfaceType::ALL)
                {
                    return Some((surface, (point + layer_geo.loc + output_geo.loc).to_f64()));
                }
            }
        }

        self.space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, point)| (surface, (point + location).to_f64()))
            })
    }

    fn output_layout_geometry(&self) -> Option<Rectangle<i32, Logical>> {
        let mut outputs = self.space.outputs();
        let first = outputs.next()?;
        let first_geo = self.space.output_geometry(first)?;

        let mut min_x = first_geo.loc.x;
        let mut min_y = first_geo.loc.y;
        let mut max_x = first_geo.loc.x + first_geo.size.w;
        let mut max_y = first_geo.loc.y + first_geo.size.h;

        for output in outputs {
            let Some(geo) = self.space.output_geometry(output) else {
                continue;
            };
            min_x = min_x.min(geo.loc.x);
            min_y = min_y.min(geo.loc.y);
            max_x = max_x.max(geo.loc.x + geo.size.w);
            max_y = max_y.max(geo.loc.y + geo.size.h);
        }

        Some(Rectangle::new(
            (min_x, min_y).into(),
            ((max_x - min_x), (max_y - min_y)).into(),
        ))
    }

    #[cfg(feature = "udev")]
    pub(crate) fn is_tty_backend(&self) -> bool {
        self.tty_control.is_some()
    }

    #[cfg(not(feature = "udev"))]
    pub(crate) fn is_tty_backend(&self) -> bool {
        false
    }

    fn output_at_screen_position(&self, pos: SmithayPoint<f64, Logical>) -> Option<Output> {
        self.space.outputs().find_map(|output| {
            let geometry = self.space.output_geometry(output)?;
            let within_x =
                pos.x >= geometry.loc.x as f64 && pos.x < (geometry.loc.x + geometry.size.w) as f64;
            let within_y =
                pos.y >= geometry.loc.y as f64 && pos.y < (geometry.loc.y + geometry.size.h) as f64;
            (within_x && within_y).then(|| output.clone())
        })
    }

    fn output_at_world_position(&self, pos: Point) -> Option<Output> {
        self.space.outputs().find_map(|output| {
            let output_state = self.output_state_for_output(output)?;
            let visible = output_state.viewport().visible_world_rect();
            let within_x = pos.x >= visible.origin.x && pos.x < visible.origin.x + visible.size.w;
            let within_y = pos.y >= visible.origin.y && pos.y < visible.origin.y + visible.size.h;
            (within_x && within_y).then(|| output.clone())
        })
    }

    fn screen_to_world_pointer_position(
        &self,
        pos: SmithayPoint<f64, Logical>,
    ) -> SmithayPoint<f64, Logical> {
        if !self.is_tty_backend() {
            return pos;
        }

        let Some(output) = self.output_at_screen_position(pos) else {
            return pos;
        };
        let Some(output_geo) = self.space.output_geometry(&output) else {
            return pos;
        };
        let Some(output_state) = self.output_state_for_output(&output) else {
            return pos;
        };

        let local = Point::new(
            pos.x - output_geo.loc.x as f64,
            pos.y - output_geo.loc.y as f64,
        );
        let world = output_state.viewport().screen_to_world(local);
        (world.x, world.y).into()
    }

    fn clamp_pointer_to_output_layout_or_viewport(
        &self,
        pos: SmithayPoint<f64, Logical>,
    ) -> SmithayPoint<f64, Logical> {
        if self.is_tty_backend() {
            let mut outputs = self.space.outputs();
            let Some(first) = outputs.next() else {
                let fallback = self.viewport().visible_world_rect();
                return (
                    pos.x.clamp(
                        fallback.origin.x,
                        fallback.origin.x + fallback.size.w.max(1.0),
                    ),
                    pos.y.clamp(
                        fallback.origin.y,
                        fallback.origin.y + fallback.size.h.max(1.0),
                    ),
                )
                    .into();
            };
            let Some(first_state) = self.output_state_for_output(first) else {
                return pos;
            };
            let first_visible = first_state.viewport().visible_world_rect();
            let mut min_x = first_visible.origin.x;
            let mut min_y = first_visible.origin.y;
            let mut max_x = first_visible.origin.x + first_visible.size.w;
            let mut max_y = first_visible.origin.y + first_visible.size.h;

            for output in outputs {
                let Some(output_state) = self.output_state_for_output(output) else {
                    continue;
                };
                let visible = output_state.viewport().visible_world_rect();
                min_x = min_x.min(visible.origin.x);
                min_y = min_y.min(visible.origin.y);
                max_x = max_x.max(visible.origin.x + visible.size.w);
                max_y = max_y.max(visible.origin.y + visible.size.h);
            }

            return (pos.x.clamp(min_x, max_x), pos.y.clamp(min_y, max_y)).into();
        }

        if let Some(output_geo) = self.output_layout_geometry() {
            let min_x = output_geo.loc.x as f64;
            let min_y = output_geo.loc.y as f64;
            let max_x = min_x + output_geo.size.w as f64;
            let max_y = min_y + output_geo.size.h as f64;
            return (pos.x.clamp(min_x, max_x), pos.y.clamp(min_y, max_y)).into();
        }

        let fallback = self.viewport().screen_size();
        (
            pos.x.clamp(0.0, fallback.w.max(1.0)),
            pos.y.clamp(0.0, fallback.h.max(1.0)),
        )
            .into()
    }

    fn absolute_pointer_position<B: InputBackend, I: AbsolutePositionEvent<B>>(
        &self,
        event: &I,
    ) -> SmithayPoint<f64, Logical> {
        if let Some(output_geo) = self.output_layout_geometry() {
            return event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
        }

        let fallback = self.viewport().screen_size();
        SmithayPoint::from((
            event.x_transformed(fallback.w.max(1.0) as i32),
            event.y_transformed(fallback.h.max(1.0) as i32),
        ))
    }

    fn viewport_pointer_anchor(&self) -> Point {
        let local_pointer = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .map(|pointer| {
                if self.is_tty_backend() {
                    let world = Point::new(pointer.x, pointer.y);
                    if let Some(output) = self.output_at_world_position(world)
                        && let Some(output_state) = self.output_state_for_output(&output)
                    {
                        return output_state.viewport().world_to_screen(world);
                    }
                    return Point::new(pointer.x, pointer.y);
                }

                let logical = self
                    .primary_output_name()
                    .and_then(|name| self.output_state_for_name(&name))
                    .map(|s| s.logical_position())
                    .unwrap_or_else(|| self.output_state.logical_position());
                Point::new(pointer.x - logical.x, pointer.y - logical.y)
            });

        let screen = self.viewport().screen_size();
        let fallback = Point::new(screen.w / 2.0, screen.h / 2.0);
        let pointer = local_pointer.unwrap_or(fallback);
        Point::new(
            pointer.x.clamp(0.0, screen.w),
            pointer.y.clamp(0.0, screen.h),
        )
    }

    fn hovered_window_snapshot_at(
        &self,
        pos: SmithayPoint<f64, Logical>,
    ) -> Option<WindowSnapshot> {
        self.surface_under(pos)
            .and_then(|(surface, _)| self.window_id_for_surface(&surface))
            .and_then(|id| self.window_snapshot_for_id(id))
    }

    // Output state management, viewport helpers, tty config helpers, and
    // state_snapshot are in output_runtime.rs
}

#[cfg(test)]
mod tests;

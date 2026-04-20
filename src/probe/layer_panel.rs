use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{wl_compositor, wl_output, wl_registry, wl_surface},
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use super::wayland_helpers::{deadline_from_hold, deadline_reached};

pub fn run(namespace: &str, hold_ms: u64) -> Result<(), Box<dyn std::error::Error>> {
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
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            match &interface[..] {
                "wl_compositor" => {
                    state.compositor =
                        Some(registry.bind::<wl_compositor::WlCompositor, _, _>(name, 1, qh, ()));
                    state.maybe_init_layer_surface(qh);
                }
                "wl_output" => {
                    state.output =
                        Some(registry.bind::<wl_output::WlOutput, _, _>(name, 3, qh, ()));
                    state.maybe_init_layer_surface(qh);
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(
                        registry.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(
                            name,
                            4,
                            qh,
                            (),
                        ),
                    );
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
        if let zwlr_layer_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
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

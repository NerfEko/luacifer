use super::*;

pub(super) fn resize_edges_from(edge: xdg_toplevel::ResizeEdge) -> ResizeEdges {
    match edge {
        xdg_toplevel::ResizeEdge::Top => ResizeEdges {
            left: false,
            right: false,
            top: true,
            bottom: false,
        },
        xdg_toplevel::ResizeEdge::Bottom => ResizeEdges {
            left: false,
            right: false,
            top: false,
            bottom: true,
        },
        xdg_toplevel::ResizeEdge::Left => ResizeEdges {
            left: true,
            right: false,
            top: false,
            bottom: false,
        },
        xdg_toplevel::ResizeEdge::TopLeft => ResizeEdges {
            left: true,
            right: false,
            top: true,
            bottom: false,
        },
        xdg_toplevel::ResizeEdge::BottomLeft => ResizeEdges {
            left: true,
            right: false,
            top: false,
            bottom: true,
        },
        xdg_toplevel::ResizeEdge::Right => ResizeEdges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        },
        xdg_toplevel::ResizeEdge::TopRight => ResizeEdges {
            left: false,
            right: true,
            top: true,
            bottom: false,
        },
        xdg_toplevel::ResizeEdge::BottomRight => ResizeEdges {
            left: false,
            right: true,
            top: false,
            bottom: true,
        },
        _ => ResizeEdges::all(),
    }
}

impl ActionTarget for EvilWm {
    fn move_window(&mut self, id: WindowId, x: f64, y: f64) -> bool {
        Self::move_window(self, id, x, y)
    }

    fn resize_window(&mut self, id: WindowId, w: f64, h: f64) -> bool {
        Self::resize_window(self, id, w, h)
    }

    fn set_window_bounds(&mut self, id: WindowId, bounds: Rect) -> bool {
        Self::set_window_bounds(self, id, bounds)
    }

    fn begin_interactive_move(&mut self, id: WindowId) -> bool {
        Self::begin_interactive_move(self, id)
    }

    fn begin_interactive_resize(&mut self, id: WindowId, edges: ResizeEdges) -> bool {
        Self::begin_interactive_resize(self, id, edges)
    }

    fn focus_window(&mut self, id: WindowId) -> bool {
        Self::focus_window(self, id)
    }

    fn clear_focus(&mut self) -> bool {
        Self::clear_focus(self)
    }

    fn close_window(&mut self, id: WindowId) -> bool {
        Self::close_window(self, id)
    }

    fn spawn_command(&mut self, command: &str) -> bool {
        if command.trim().is_empty() {
            return false;
        }
        super::spawn_client(command, &self.socket_name, &self.ipc_socket_path);
        true
    }

    fn pan_canvas(&mut self, dx: f64, dy: f64) {
        self.pan_all_viewports(crate::canvas::Vec2::new(dx, dy));
    }

    fn zoom_canvas(&mut self, factor: f64) -> Result<(), ConfigError> {
        if factor <= 0.0 {
            return Err(ConfigError::Validation(
                "hook action zoom_canvas requires factor > 0".into(),
            ));
        }
        let screen = self.viewport().screen_size();
        self.zoom_all_viewports_at_primary(Point::new(screen.w / 2.0, screen.h / 2.0), factor);
        Ok(())
    }
}

pub(super) fn format_keyspec(key: &str, modifiers: ModifierSet) -> String {
    let mut parts = Vec::new();
    if modifiers.logo {
        parts.push("Super".to_string());
    }
    if modifiers.ctrl {
        parts.push("Ctrl".to_string());
    }
    if modifiers.alt {
        parts.push("Alt".to_string());
    }
    if modifiers.shift {
        parts.push("Shift".to_string());
    }
    parts.push(key.to_string());
    parts.join("+")
}

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
fn output_snapshot_for_render(state: &EvilWm, output: &Output) -> Option<OutputSnapshot> {
    let output_state = state.output_state_for_output(output)?;
    let viewport = output_state.viewport();
    let logical_position = output_state.logical_position();
    Some(OutputSnapshot {
        id: output.name(),
        logical_x: logical_position.x,
        logical_y: logical_position.y,
        viewport: ViewportSnapshot {
            x: viewport.world_origin().x,
            y: viewport.world_origin().y,
            zoom: viewport.zoom(),
            screen_w: viewport.screen_size().w,
            screen_h: viewport.screen_size().h,
            visible_world: viewport.visible_world_rect(),
        },
    })
}

#[cfg(feature = "lua")]
fn draw_commands_for_output(
    state: &mut EvilWm,
    output: &Output,
    hook_name: &str,
) -> Vec<DrawCommand> {
    let Some(output_snapshot) = output_snapshot_for_render(state, output) else {
        return Vec::new();
    };
    if let Some(result) = state.with_live_lua(|hooks, state| {
        hooks.draw_commands_for_output(state, hook_name, &output_snapshot)
    }) {
        match result {
            Ok(commands) => commands,
            Err(error) => {
                state.record_live_hook_error(hook_name, &error);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    }
}

#[cfg(not(feature = "lua"))]
fn draw_commands_for_output(
    _state: &mut EvilWm,
    _output: &Output,
    _hook_name: &str,
) -> Vec<DrawCommand> {
    Vec::new()
}

pub(crate) fn configured_draw_stack(state: &EvilWm) -> &[DrawLayer] {
    state
        .config
        .as_ref()
        .map(|config| config.draw.stack.as_slice())
        .unwrap_or(&[
            DrawLayer::Background,
            DrawLayer::Windows,
            DrawLayer::WindowOverlay,
            DrawLayer::Popups,
            DrawLayer::Overlay,
            DrawLayer::Cursor,
        ])
}

pub(crate) fn render_stack_front_to_back(state: &EvilWm) -> Vec<DrawLayer> {
    configured_draw_stack(state).iter().rev().copied().collect()
}

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
fn draw_rect_to_physical(
    state: &EvilWm,
    output: &Output,
    space: DrawSpace,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Option<Rectangle<i32, Physical>> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let viewport = state.output_state_for_output(output)?.viewport();
    let (screen_x, screen_y, screen_w, screen_h) = match space {
        DrawSpace::Screen => (x, y, w, h),
        DrawSpace::World => {
            let top_left = viewport.world_to_screen(Point::new(x, y));
            let bottom_right = viewport.world_to_screen(Point::new(x + w, y + h));
            (
                top_left.x,
                top_left.y,
                bottom_right.x - top_left.x,
                bottom_right.y - top_left.y,
            )
        }
    };

    let screen = viewport.screen_size();
    let unclipped_right = screen_x + screen_w;
    let unclipped_bottom = screen_y + screen_h;

    if unclipped_right <= 0.0
        || unclipped_bottom <= 0.0
        || screen_x >= screen.w
        || screen_y >= screen.h
    {
        return None;
    }

    let clipped_left = screen_x.max(0.0);
    let clipped_top = screen_y.max(0.0);
    let clipped_right = unclipped_right.min(screen.w);
    let clipped_bottom = unclipped_bottom.min(screen.h);

    if clipped_right <= clipped_left || clipped_bottom <= clipped_top {
        return None;
    }

    let width = (clipped_right - clipped_left).round().max(1.0) as i32;
    let height = (clipped_bottom - clipped_top).round().max(1.0) as i32;
    let left = clipped_left.round() as i32;
    let top = clipped_top.round() as i32;

    Some(Rectangle::<i32, Physical>::new(
        (left, top).into(),
        (width, height).into(),
    ))
}

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
fn stroke_rects(
    rect: Rectangle<i32, Physical>,
    width: i32,
    outer: i32,
) -> Vec<Rectangle<i32, Physical>> {
    if rect.size.w <= 0 || rect.size.h <= 0 || width <= 0 || outer < 0 {
        return Vec::new();
    }
    let x = rect.loc.x - outer;
    let y = rect.loc.y - outer;
    let w = rect.size.w + outer * 2;
    let h = rect.size.h + outer * 2;
    vec![
        Rectangle::<i32, Physical>::new((x, y).into(), (w, width).into()),
        Rectangle::<i32, Physical>::new((x, y + h - width).into(), (w, width).into()),
        Rectangle::<i32, Physical>::new((x, y).into(), (width, h).into()),
        Rectangle::<i32, Physical>::new((x + w - width, y).into(), (width, h).into()),
    ]
}

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
pub(crate) fn solid_elements_from_draw_commands(
    state: &mut EvilWm,
    output: &Output,
    hook_name: &str,
) -> Vec<SolidColorRenderElement> {
    let commands = draw_commands_for_output(state, output, hook_name);
    let mut elements = Vec::with_capacity(commands.len());
    for command in commands {
        match command {
            DrawCommand::Rect {
                space,
                x,
                y,
                w,
                h,
                color,
            } => {
                if let Some(rect) = draw_rect_to_physical(state, output, space, x, y, w, h) {
                    elements.push(SolidColorRenderElement::new(
                        Id::new(),
                        rect,
                        0usize,
                        color,
                        Kind::Unspecified,
                    ));
                }
            }
            DrawCommand::StrokeRect {
                space,
                x,
                y,
                w,
                h,
                width,
                outer,
                color,
            } => {
                if let Some(rect) = draw_rect_to_physical(state, output, space, x, y, w, h) {
                    for stroke in stroke_rects(
                        rect,
                        width.round().max(1.0) as i32,
                        outer.round().max(0.0) as i32,
                    ) {
                        elements.push(SolidColorRenderElement::new(
                            Id::new(),
                            stroke,
                            0usize,
                            color,
                            Kind::Unspecified,
                        ));
                    }
                }
            }
        }
    }
    elements
}

pub(crate) fn lock_overlay_elements(
    state: &EvilWm,
    output: &Output,
) -> Vec<SolidColorRenderElement> {
    if !state.session_locked {
        return Vec::new();
    }

    let size = output
        .current_mode()
        .map(|mode| mode.size)
        .unwrap_or_else(|| {
            let screen = state
                .output_state_for_output(output)
                .map(|output_state| output_state.viewport().screen_size())
                .unwrap_or_else(|| state.output_state.viewport().screen_size());
            smithay::utils::Size::from((screen.w.round() as i32, screen.h.round() as i32))
        });

    vec![SolidColorRenderElement::new(
        Id::new(),
        Rectangle::<i32, Physical>::new((0, 0).into(), size),
        0usize,
        [0.02, 0.02, 0.03, 0.92],
        Kind::Unspecified,
    )]
}

/// Write a PPM (P6 binary) screenshot and return the total bytes written.
///
/// PPM is chosen deliberately: it requires no external crate, is trivially
/// verifiable in tests, and needs no decoder to inspect in CI artifacts.
pub(super) fn write_ppm_screenshot(
    path: &std::path::Path,
    size: smithay::utils::Size<i32, Physical>,
    pixels: &[u8],
    flipped: bool,
) -> Result<usize, std::io::Error> {
    use std::io::Write;

    let width = size.w.max(1) as usize;
    let height = size.h.max(1) as usize;
    let stride = width * 4;
    let header = format!("P6\n{} {}\n255\n", width, height);
    let mut written = header.len();
    let mut file = std::fs::File::create(path)?;
    file.write_all(header.as_bytes())?;

    for row in 0..height {
        let source_row = if flipped { height - 1 - row } else { row };
        let start = source_row * stride;
        let end = start + stride;
        let row_pixels = &pixels[start..end];
        for chunk in row_pixels.chunks_exact(4) {
            file.write_all(&chunk[0..3])?;
            written += 3;
        }
    }

    Ok(written)
}

pub(crate) struct SplitSpaceElements<T> {
    pub windows: Vec<T>,
    pub popups: Vec<T>,
}

#[cfg(any(feature = "winit", feature = "x11"))]
pub(crate) fn build_winit_space_elements(
    state: &EvilWm,
    renderer: &mut GlesRenderer,
    output: &Output,
) -> Result<
    SplitSpaceElements<
        smithay::desktop::space::SpaceRenderElements<
            GlesRenderer,
            smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
        >,
    >,
    smithay::output::OutputNoMode,
> {
    let output_scale = output.current_scale().fractional_scale();
    let output_geo = state
        .space
        .output_geometry(output)
        .ok_or(smithay::output::OutputNoMode)?;
    let scale = smithay::utils::Scale::from(output_scale);

    let mut windows = Vec::new();
    let mut popups = Vec::new();

    let lower_layers = {
        let layer_map = layer_map_for_output(output);
        let (lower, upper): (Vec<DesktopLayerSurface>, Vec<DesktopLayerSurface>) =
            layer_map.layers().rev().cloned().partition(|surface| {
                matches!(surface.layer(), WlrLayer::Background | WlrLayer::Bottom)
            });

        popups.extend(
            upper
                .into_iter()
                .filter_map(|surface| {
                    layer_map
                        .layer_geometry(&surface)
                        .map(|geo| (geo.loc, surface))
                })
                .flat_map(|(loc, surface)| {
                    smithay::backend::renderer::element::AsRenderElements::<GlesRenderer>::render_elements::<
                        smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<
                            GlesRenderer,
                        >,
                    >(
                        &surface,
                        renderer,
                        loc.to_physical_precise_round(output_scale),
                        scale,
                        1.0,
                    )
                    .into_iter()
                    .map(smithay::desktop::space::SpaceRenderElements::Surface)
                }),
        );

        lower
    };

    for window in state.space.elements().rev() {
        let Some(space_bbox) = state.space.element_bbox(window) else {
            continue;
        };
        if !output_geo.overlaps(space_bbox) {
            continue;
        }

        let Some(element_location) = state.space.element_location(window) else {
            continue;
        };
        let render_location = element_location - window.geometry().loc - output_geo.loc;
        let physical_location = render_location.to_physical_precise_round(output_scale);

        match window.underlying_surface() {
            smithay::desktop::WindowSurface::Wayland(toplevel) => {
                windows.extend(
                    smithay::backend::renderer::element::surface::render_elements_from_surface_tree::<
                                _,
                                smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
                            >(
                        renderer,
                        toplevel.wl_surface(),
                        physical_location,
                        scale,
                        1.0,
                        Kind::Unspecified,
                    )
                    .into_iter()
                    .map(|element| {
                        smithay::desktop::space::SpaceRenderElements::Element(smithay::backend::renderer::element::Wrap::from(element))
                    }),
                );

                popups.extend(
                    PopupManager::popups_for_surface(toplevel.wl_surface())
                        .flat_map(|(popup, popup_offset)| {
                            let popup_render_location =
                                element_location + popup_offset - popup.geometry().loc - output_geo.loc;
                            smithay::backend::renderer::element::surface::render_elements_from_surface_tree::<
                                _,
                                smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
                            >(
                                renderer,
                                popup.wl_surface(),
                                popup_render_location.to_physical_precise_round(output_scale),
                                scale,
                                1.0,
                                Kind::Unspecified,
                            )
                        })
                        .map(|element| {
                            smithay::desktop::space::SpaceRenderElements::Element(smithay::backend::renderer::element::Wrap::from(element))
                        }),
                );
            }
            #[cfg(feature = "xwayland")]
            smithay::desktop::WindowSurface::X11(surface) => {
                let elements = smithay::backend::renderer::element::AsRenderElements::<GlesRenderer>::render_elements::<
                    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<
                        GlesRenderer,
                    >,
                >(surface, renderer, physical_location, scale, 1.0)
                .into_iter()
                .map(|element| {
                    smithay::desktop::space::SpaceRenderElements::Element(smithay::backend::renderer::element::Wrap::from(element))
                });

                if surface.is_override_redirect()
                    || surface.is_popup()
                    || surface.is_transient_for().is_some()
                {
                    popups.extend(elements);
                } else {
                    windows.extend(elements);
                }
            }
        }
    }

    windows.extend(
        lower_layers
            .into_iter()
            .filter_map(|surface| {
                layer_map_for_output(output)
                    .layer_geometry(&surface)
                    .map(|geo| (geo.loc, surface))
            })
            .flat_map(|(loc, surface)| {
                smithay::backend::renderer::element::AsRenderElements::<GlesRenderer>::render_elements::<
                    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<
                        GlesRenderer,
                    >,
                >(
                    &surface,
                    renderer,
                    loc.to_physical_precise_round(output_scale),
                    scale,
                    1.0,
                )
                .into_iter()
                .map(smithay::desktop::space::SpaceRenderElements::Surface)
            }),
    );

    Ok(SplitSpaceElements { windows, popups })
}

#[cfg(feature = "udev")]
fn output_visible_world_geometry(
    state: &EvilWm,
    output: &Output,
) -> Option<Rectangle<i32, Logical>> {
    let viewport = state.output_state_for_output(output)?.viewport();
    let top_left = viewport.screen_to_world(Point::new(0.0, 0.0));
    let bottom_right = viewport.screen_to_world(Point::new(
        viewport.screen_size().w,
        viewport.screen_size().h,
    ));

    let left = top_left.x.floor() as i32;
    let top = top_left.y.floor() as i32;
    let right = bottom_right.x.ceil() as i32;
    let bottom = bottom_right.y.ceil() as i32;

    Some(Rectangle::new(
        (left, top).into(),
        ((right - left).max(1), (bottom - top).max(1)).into(),
    ))
}

#[cfg(feature = "udev")]
pub(crate) fn build_live_space_elements(
    state: &EvilWm,
    renderer: &mut GlesRenderer,
    output: &Output,
) -> SplitSpaceElements<
    smithay::desktop::space::SpaceRenderElements<
        GlesRenderer,
        RescaleRenderElement<
            smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
        >,
    >,
> {
    let Some(region) = output_visible_world_geometry(state, output) else {
        return SplitSpaceElements {
            windows: Vec::new(),
            popups: Vec::new(),
        };
    };

    let output_scale = output.current_scale().fractional_scale();
    let output_scale_factor = smithay::utils::Scale::from(output_scale);
    let Some(output_state) = state.output_state_for_output(output) else {
        return SplitSpaceElements {
            windows: Vec::new(),
            popups: Vec::new(),
        };
    };
    let viewport_scale = smithay::utils::Scale::from(output_state.viewport().zoom());

    let mut windows = Vec::new();
    let mut popups = Vec::new();

    for window in state.space.elements().rev() {
        let Some(space_bbox) = state.space.element_bbox(window) else {
            continue;
        };
        if !region.overlaps(space_bbox) {
            continue;
        }

        let Some(element_location) = state.space.element_location(window) else {
            continue;
        };
        let render_location = element_location - window.geometry().loc - region.loc;
        let physical_location = render_location.to_physical_precise_round(output_scale_factor);

        match window.underlying_surface() {
            smithay::desktop::WindowSurface::Wayland(toplevel) => {
                windows.extend(
                    smithay::backend::renderer::element::surface::render_elements_from_surface_tree::<
                                _,
                                smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
                            >(
                        renderer,
                        toplevel.wl_surface(),
                        physical_location,
                        output_scale_factor,
                        1.0,
                        Kind::Unspecified,
                    )
                    .into_iter()
                    .map(|element| {
                        RescaleRenderElement::from_element(element, (0, 0).into(), viewport_scale)
                    })
                    .map(|element| {
                        smithay::desktop::space::SpaceRenderElements::Element(smithay::backend::renderer::element::Wrap::from(element))
                    }),
                );

                popups.extend(
                    PopupManager::popups_for_surface(toplevel.wl_surface())
                        .flat_map(|(popup, popup_offset)| {
                            let popup_render_location =
                                element_location + popup_offset - popup.geometry().loc - region.loc;
                            smithay::backend::renderer::element::surface::render_elements_from_surface_tree::<
                                _,
                                smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
                            >(
                                renderer,
                                popup.wl_surface(),
                                popup_render_location.to_physical_precise_round(output_scale_factor),
                                output_scale_factor,
                                1.0,
                                Kind::Unspecified,
                            )
                        })
                        .map(|element| {
                            RescaleRenderElement::from_element(element, (0, 0).into(), viewport_scale)
                        })
                        .map(|element| {
                            smithay::desktop::space::SpaceRenderElements::Element(smithay::backend::renderer::element::Wrap::from(element))
                        }),
                );
            }
            #[cfg(feature = "xwayland")]
            smithay::desktop::WindowSurface::X11(surface) => {
                let elements = smithay::backend::renderer::element::AsRenderElements::<GlesRenderer>::render_elements::<
                    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<
                        GlesRenderer,
                    >,
                >(
                    surface,
                    renderer,
                    physical_location,
                    output_scale_factor,
                    1.0,
                )
                .into_iter()
                .map(|element| {
                    RescaleRenderElement::from_element(element, (0, 0).into(), viewport_scale)
                })
                .map(|element| {
                    smithay::desktop::space::SpaceRenderElements::Element(smithay::backend::renderer::element::Wrap::from(element))
                });

                if surface.is_override_redirect()
                    || surface.is_popup()
                    || surface.is_transient_for().is_some()
                {
                    popups.extend(elements);
                } else {
                    windows.extend(elements);
                }
            }
        }
    }

    SplitSpaceElements { windows, popups }
}

#[cfg(feature = "udev")]
pub(crate) fn build_cursor_elements(
    state: &EvilWm,
    output: &Output,
) -> Vec<SolidColorRenderElement> {
    let Some(pointer) = state.seat.get_pointer() else {
        return Vec::new();
    };
    let pos = pointer.current_location();

    let (local_x, local_y) = if state.is_tty_backend() {
        let Some(output_state) = state.output_state_for_output(output) else {
            return Vec::new();
        };
        let visible = output_state.viewport().visible_world_rect();
        if pos.x < visible.origin.x
            || pos.y < visible.origin.y
            || pos.x >= visible.origin.x + visible.size.w
            || pos.y >= visible.origin.y + visible.size.h
        {
            return Vec::new();
        }
        let local = output_state
            .viewport()
            .world_to_screen(Point::new(pos.x, pos.y));
        (local.x.round() as i32, local.y.round() as i32)
    } else {
        let Some(output_geo) = state.space.output_geometry(output) else {
            return Vec::new();
        };
        if pos.x < output_geo.loc.x as f64
            || pos.y < output_geo.loc.y as f64
            || pos.x >= (output_geo.loc.x + output_geo.size.w) as f64
            || pos.y >= (output_geo.loc.y + output_geo.size.h) as f64
        {
            return Vec::new();
        }
        (
            pos.x.round() as i32 - output_geo.loc.x,
            pos.y.round() as i32 - output_geo.loc.y,
        )
    };

    vec![SolidColorRenderElement::new(
        Id::new(),
        Rectangle::<i32, Physical>::new((local_x, local_y).into(), (10, 10).into()),
        0usize,
        [0.95, 0.95, 0.98, 0.95],
        Kind::Cursor,
    )]
}

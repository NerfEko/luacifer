use super::*;

impl EvilWm {
    pub(crate) fn handle_pointer_position(
        &mut self,
        pos: SmithayPoint<f64, Logical>,
        serial: Serial,
        time: u32,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let under = self.surface_under(pos);
        let pointer_grabbed = pointer.is_grabbed();
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial,
                time,
            },
        );
        pointer.frame(self);

        let current = Point::new(pos.x, pos.y);
        let previous_focus = self.focus_stack.focused();
        let hovered_window = if pointer_grabbed {
            None
        } else {
            self.hovered_window_snapshot_at(pos)
        };
        self.advance_active_interactive_op(current);

        if !pointer_grabbed {
            let _ = self.try_live_resolve_focus(ResolveFocusRequest {
                reason: "pointer_motion",
                window: hovered_window.as_ref(),
                previous: previous_focus,
                pointer: Some(current),
                button: None,
                pressed: None,
                modifiers: Some(self.current_modifier_set()),
            });
        }
    }

    pub(crate) fn apply_trackpad_swipe(&mut self, delta: SmithayPoint<f64, Logical>) {
        self.pan_all_viewports(crate::canvas::Vec2::new(-delta.x, -delta.y));
    }

    pub(crate) fn begin_trackpad_pinch(&mut self) {
        self.trackpad_pinch_scale = Some(1.0);
    }

    pub(crate) fn apply_trackpad_pinch(&mut self, delta: SmithayPoint<f64, Logical>, absolute_scale: f64) {
        self.apply_trackpad_swipe(delta);

        if let Some(relative) =
            pinch_relative_factor(&mut self.trackpad_pinch_scale, absolute_scale)
            && (relative - 1.0).abs() > f64::EPSILON
        {
            let anchor = self.viewport_pointer_anchor();
            self.zoom_all_viewports_at_primary(anchor, relative);
        }
    }

    pub(crate) fn end_trackpad_pinch(&mut self) {
        self.trackpad_pinch_scale = None;
    }

    #[cfg(feature = "lua")]
    pub(crate) fn live_hook_exists(&mut self, hook_name: &str) -> bool {
        self.with_live_lua(|hooks, _| hooks.has_hook(hook_name))
            .unwrap_or(Ok(false))
            .unwrap_or_else(|error| {
                eprintln!("{}", format_live_hook_error(hook_name, &error));
                false
            })
    }

    pub(crate) fn warn_missing_move_update_hook(&mut self) {
        if self.warned_missing_move_update_hook {
            return;
        }
        self.warned_missing_move_update_hook = true;
        eprintln!(
            "interactive move requested, but no Lua move_update hook is installed; drag will do nothing"
        );
    }

    pub(crate) fn warn_missing_resize_update_hook(&mut self) {
        if self.warned_missing_resize_update_hook {
            return;
        }
        self.warned_missing_resize_update_hook = true;
        eprintln!(
            "interactive resize requested, but no Lua resize_update hook is installed; resize will do nothing"
        );
    }

    pub(crate) fn current_modifier_set(&self) -> ModifierSet {
        self.seat
            .get_keyboard()
            .map(|keyboard| modifier_set_from(&keyboard.modifier_state()))
            .unwrap_or(ModifierSet {
                ctrl: false,
                alt: false,
                shift: false,
                logo: false,
            })
    }

    pub(crate) fn handle_resolved_key(&mut self, key: &str, modifiers: ModifierSet) -> bool {
        let keyspec = format_keyspec(key, modifiers);
        let bound_action = self.bindings.resolve(key, modifiers);
        let intercepted = bound_action.is_some();
        let hook_handled = self.trigger_live_key(keyspec);

        if !hook_handled && let Some(action) = bound_action {
            self.handle_action(action);
        }

        intercepted
    }

    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let key_state = event.state();
                let Some(keyboard) = self.seat.get_keyboard() else {
                    return;
                };
                keyboard.input::<(), _>(
                    self,
                    event.key_code(),
                    key_state,
                    serial,
                    time,
                    |state, modifiers, handle| {
                        if key_state == KeyState::Pressed {
                            let key = handle
                                .raw_latin_sym_or_raw_current_sym()
                                .map(xkb::keysym_get_name)
                                .unwrap_or_else(|| xkb::keysym_get_name(handle.modified_sym()));
                            let key = crate::input::bindings::normalize_key(&key);
                            let modifier_set = modifier_set_from(modifiers);
                            #[cfg(feature = "udev")]
                            if let Some(control) = default_tty_control_action(&key, modifier_set)
                                && let Some(callback) = state.tty_control.clone()
                            {
                                let control_name = match control {
                                    TtyControlAction::Quit => "quit".to_string(),
                                    TtyControlAction::SwitchVt(vt) => format!("switch_vt_{vt}"),
                                };
                                state.emit_event(
                                    "key_pressed",
                                    serde_json::json!({
                                        "key": key,
                                        "modifiers": modifier_set_json(modifier_set),
                                        "intercepted": true,
                                        "tty_control": control_name,
                                    }),
                                );
                                callback.borrow_mut()(control);
                                return FilterResult::Intercept(());
                            }
                            if state.session_locked {
                                state.emit_event(
                                    "key_pressed",
                                    serde_json::json!({
                                        "key": key,
                                        "modifiers": modifier_set_json(modifier_set),
                                        "intercepted": true,
                                        "session_locked": true,
                                    }),
                                );
                                return FilterResult::Intercept(());
                            }
                            let intercepted = state.handle_resolved_key(&key, modifier_set);
                            state.emit_event(
                                "key_pressed",
                                serde_json::json!({
                                    "key": key,
                                    "modifiers": modifier_set_json(modifier_set),
                                    "intercepted": intercepted,
                                }),
                            );
                            if intercepted {
                                return FilterResult::Intercept(());
                            }
                        }
                        FilterResult::Forward
                    },
                );
            }
            InputEvent::PointerMotion { event, .. } => {
                if self.session_locked {
                    return;
                }
                let serial = SERIAL_COUNTER.next_serial();
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };
                let pos = if self.is_tty_backend() {
                    let current = pointer.current_location();
                    let current_world = Point::new(current.x, current.y);
                    let zoom = self
                        .output_at_world_position(current_world)
                        .and_then(|output| self.output_state_for_output(&output))
                        .map(|state| state.viewport().zoom())
                        .unwrap_or_else(|| self.viewport().zoom());
                    self.clamp_pointer_to_output_layout_or_viewport(
                        (
                            current.x + event.delta().x / zoom,
                            current.y + event.delta().y / zoom,
                        )
                            .into(),
                    )
                } else {
                    self.clamp_pointer_to_output_layout_or_viewport(
                        pointer.current_location() + event.delta(),
                    )
                };
                self.handle_pointer_position(pos, serial, event.time_msec());
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                if self.session_locked {
                    return;
                }
                let pos = if self.is_tty_backend() {
                    self.clamp_pointer_to_output_layout_or_viewport(
                        self.screen_to_world_pointer_position(
                            self.absolute_pointer_position(&event),
                        ),
                    )
                } else {
                    self.absolute_pointer_position(&event)
                };
                let serial = SERIAL_COUNTER.next_serial();
                self.handle_pointer_position(pos, serial, event.time_msec());
            }
            InputEvent::PointerButton { event, .. } => {
                if self.session_locked {
                    return;
                }
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let button_state = event.state();

                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };

                if ButtonState::Pressed == button_state {
                    self.last_pointer_button_pressed = Some(button);
                }

                let current_location = pointer.current_location();
                let modifiers = self.current_modifier_set();
                let mut hovered_window_id = None;
                if !pointer.is_grabbed() {
                    // Re-assert the current hover target before forwarding the button so
                    // clicks always use the same hit-test path as motion/enter handling.
                    pointer.motion(
                        self,
                        self.surface_under(current_location),
                        &MotionEvent {
                            location: current_location,
                            serial,
                            time: event.time_msec(),
                        },
                    );
                    pointer.frame(self);
                }

                let mut suppress_press = false;
                if ButtonState::Pressed == button_state && !pointer.is_grabbed() && button == 274 {
                    self.active_interactive_op = Some(ActiveInteractiveOp::new(
                        ActiveInteractiveKind::PanCanvas,
                        WindowId(0),
                        Point::new(current_location.x, current_location.y),
                        None,
                        Some(button),
                    ));
                    suppress_press = true;
                    self.suppress_pointer_button_release = Some(button);
                } else if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    let previous_focus = self.focus_stack.focused();
                    let pointer_point = Point::new(current_location.x, current_location.y);
                    let hovered_window = self.hovered_window_snapshot_at(current_location);
                    hovered_window_id = hovered_window.as_ref().map(|window| window.id);
                    let had_interactive_before = self.active_interactive_op.is_some();
                    let modifier_resize_click =
                        hovered_window.is_some() && modifiers.logo && button != 272;

                    let handled = self.try_live_resolve_focus(ResolveFocusRequest {
                        reason: "pointer_button",
                        window: hovered_window.as_ref(),
                        previous: previous_focus,
                        pointer: Some(pointer_point),
                        button: Some(button),
                        pressed: Some(true),
                        modifiers: Some(modifiers),
                    });
                    let interactive_active = self.active_interactive_op.is_some();
                    let started_interactive_via_hook =
                        !had_interactive_before && interactive_active;
                    suppress_press =
                        started_interactive_via_hook || (modifier_resize_click && handled);
                    if suppress_press {
                        self.suppress_pointer_button_release = Some(button);
                    }
                    if !handled {
                        if let Some(window) = hovered_window {
                            let _ = self.focus_window(WindowId(window.id));
                        } else {
                            let _ = self.clear_focus();
                        }
                    }
                }

                let suppress_release = ButtonState::Released == button_state
                    && self.suppress_pointer_button_release == Some(button);

                if !suppress_press && !suppress_release {
                    pointer.button(
                        self,
                        &ButtonEvent {
                            button,
                            state: button_state,
                            serial,
                            time: event.time_msec(),
                        },
                    );
                }
                pointer.frame(self);
                self.emit_event(
                    "pointer_button",
                    serde_json::json!({
                        "button": button,
                        "state": match button_state {
                            ButtonState::Pressed => "pressed",
                            ButtonState::Released => "released",
                        },
                        "pointer": {
                            "x": current_location.x,
                            "y": current_location.y,
                        },
                        "modifiers": modifier_set_json(modifiers),
                        "hovered_window_id": hovered_window_id,
                        "suppressed": suppress_press || suppress_release,
                    }),
                );

                if ButtonState::Released == button_state
                    && self.last_pointer_button_pressed == Some(button)
                {
                    self.last_pointer_button_pressed = None;
                }
                if ButtonState::Released == button_state
                    && self.suppress_pointer_button_release == Some(button)
                {
                    self.suppress_pointer_button_release = None;
                }

                if ButtonState::Released == button_state {
                    let pointer_pos = self
                        .seat
                        .get_pointer()
                        .map(|pointer| pointer.current_location())
                        .unwrap_or_else(|| (0.0, 0.0).into());
                    self.finish_active_interactive_op(
                        button,
                        Point::new(pointer_pos.x, pointer_pos.y),
                    );
                }
            }
            InputEvent::PointerAxis { event, .. } => {
                if self.session_locked {
                    return;
                }
                let source = event.source();
                // v120 scroll standard: 120 discrete units per wheel notch.
                // Convert to pixels using a fixed pixels-per-notch ratio.
                const SCROLL_PIXELS_PER_NOTCH: f64 = 15.0;
                const V120_UNITS_PER_NOTCH: f64 = 120.0;

                let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0)
                        * SCROLL_PIXELS_PER_NOTCH / V120_UNITS_PER_NOTCH
                });
                let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0)
                        * SCROLL_PIXELS_PER_NOTCH / V120_UNITS_PER_NOTCH
                });
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                if self.current_modifier_set().logo {
                    let zoom_delta = vertical_amount_discrete.unwrap_or(vertical_amount);
                    if zoom_delta != 0.0 {
                        let zoom_step =
                            self.config.as_ref().map_or(1.2, |cfg| cfg.canvas.zoom_step);
                        let factor = if zoom_delta < 0.0 {
                            zoom_step
                        } else {
                            1.0 / zoom_step
                        };
                        let anchor = self.viewport_pointer_anchor();
                        self.zoom_all_viewports_at_primary(anchor, factor);
                        return;
                    }
                }

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
                    }
                }
                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            InputEvent::GestureSwipeBegin { event, .. } => {
                if self.session_locked {
                    return;
                }
                self.emit_event(
                    "gesture",
                    serde_json::json!({
                        "kind": "swipe_begin",
                        "fingers": event.fingers(),
                    }),
                );
                self.trigger_live_gesture(
                    "swipe_begin",
                    event.fingers(),
                    crate::canvas::Vec2::new(0.0, 0.0),
                    None,
                );
            }
            InputEvent::GestureSwipeUpdate { event, .. } => {
                if self.session_locked {
                    return;
                }
                let delta = event.delta();
                self.apply_trackpad_swipe(delta);
                self.emit_event(
                    "gesture",
                    serde_json::json!({
                        "kind": "swipe_update",
                        "delta": { "x": delta.x, "y": delta.y },
                    }),
                );
                self.trigger_live_gesture(
                    "swipe_update",
                    0,
                    crate::canvas::Vec2::new(delta.x, delta.y),
                    None,
                );
            }
            InputEvent::GestureSwipeEnd { .. } => {
                if self.session_locked {
                    return;
                }
                self.emit_event("gesture", serde_json::json!({ "kind": "swipe_end" }));
                self.trigger_live_gesture("swipe_end", 0, crate::canvas::Vec2::new(0.0, 0.0), None);
            }
            InputEvent::GesturePinchBegin { event, .. } => {
                if self.session_locked {
                    return;
                }
                self.begin_trackpad_pinch();
                self.emit_event(
                    "gesture",
                    serde_json::json!({
                        "kind": "pinch_begin",
                        "fingers": event.fingers(),
                        "scale": 1.0,
                    }),
                );
                self.trigger_live_gesture(
                    "pinch_begin",
                    event.fingers(),
                    crate::canvas::Vec2::new(0.0, 0.0),
                    Some(1.0),
                );
            }
            InputEvent::GesturePinchUpdate { event, .. } => {
                if self.session_locked {
                    return;
                }
                let delta = event.delta();
                self.apply_trackpad_pinch(delta, event.scale());
                self.emit_event(
                    "gesture",
                    serde_json::json!({
                        "kind": "pinch_update",
                        "delta": { "x": delta.x, "y": delta.y },
                        "scale": event.scale(),
                    }),
                );
                self.trigger_live_gesture(
                    "pinch_update",
                    0,
                    crate::canvas::Vec2::new(delta.x, delta.y),
                    Some(event.scale()),
                );
            }
            InputEvent::GesturePinchEnd { .. } => {
                if self.session_locked {
                    return;
                }
                self.end_trackpad_pinch();
                self.emit_event("gesture", serde_json::json!({ "kind": "pinch_end" }));
                self.trigger_live_gesture("pinch_end", 0, crate::canvas::Vec2::new(0.0, 0.0), None);
            }
            _ => {}
        }
        self.request_redraw();
    }
}

#[cfg(test)]
pub(super) fn apply_trackpad_swipe_to_viewport(
    viewport: &mut crate::canvas::Viewport,
    delta: SmithayPoint<f64, Logical>,
) {
    viewport.pan_world(crate::canvas::Vec2::new(-delta.x, -delta.y));
}

pub(super) fn pinch_relative_factor(
    previous_scale: &mut Option<f64>,
    absolute_scale: f64,
) -> Option<f64> {
    let clamped_scale = absolute_scale.max(0.0001);
    let previous = previous_scale.replace(clamped_scale)?;
    Some(clamped_scale / previous.max(0.0001))
}

fn modifier_set_json(modifiers: ModifierSet) -> serde_json::Value {
    serde_json::json!({
        "ctrl": modifiers.ctrl,
        "alt": modifiers.alt,
        "shift": modifiers.shift,
        "super": modifiers.logo,
    })
}

fn modifier_set_from(modifiers: &ModifiersState) -> ModifierSet {
    ModifierSet {
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
        logo: modifiers.logo,
    }
}

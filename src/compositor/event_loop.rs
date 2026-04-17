use super::*;

pub fn run_winit(options: RuntimeOptions) -> Result<(), Box<dyn std::error::Error>> {
    let startup_commands = startup_commands(&options);
    let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new()?;
    let display: Display<EvilWm> = Display::new()?;
    let mut state = EvilWm::new(
        &mut event_loop,
        display,
        options.config_path.clone(),
        options.config,
    )?;

    let (mut backend, winit) = winit::init()?;

    let mode = OutputMode {
        size: backend.window_size(),
        refresh: 60_000,
    };
    let output = Output::new(
        "evilwm".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "evilwm".into(),
            model: "winit".into(),
        },
    );
    let _global = output.create_global::<EvilWm>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));
    state.register_output_state(
        output.name(),
        Point::new(0.0, 0.0),
        Size::new(mode.size.w as f64, mode.size.h as f64),
    );
    state.output_state = state
        .output_state_for_output(&output)
        .cloned()
        .unwrap_or_else(|| {
            OutputState::new(
                output.name(),
                Point::new(0.0, 0.0),
                Size::new(mode.size.w as f64, mode.size.h as f64),
            )
        });
    state.notify_output_management_state();

    #[cfg(feature = "xwayland")]
    spawn_xwayland(&state.display_handle, &event_loop.handle());

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                output.change_current_state(
                    Some(OutputMode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    None,
                    None,
                );
                state.sync_output_state(
                    &output.name(),
                    Point::new(0.0, 0.0),
                    Size::new(size.w as f64, size.h as f64),
                );
                if let Some(output_state) = state.output_state_for_output(&output).cloned() {
                    state.output_state = output_state;
                }
                state.notify_output_management_state();
                state.arrange_layer_surfaces_for_output(&output);
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                let size = backend.window_size();
                let damage = Rectangle::from_size(size);
                {
                    let Ok((renderer, mut framebuffer)) = backend.bind() else {
                        eprintln!("winit backend bind failed during redraw");
                        state.request_redraw();
                        return;
                    };
                    let mut background_elements = Some(solid_elements_from_draw_commands(
                        state,
                        &output,
                        "draw_background",
                    ));
                    let mut overlay_elements = Some({
                        let mut elements =
                            solid_elements_from_draw_commands(state, &output, "draw_overlay");
                        elements.extend(lock_overlay_elements(state, &output));
                        elements
                    });
                    let mut space_elements = Some(
                        match smithay::desktop::space::space_render_elements(
                            renderer,
                            [&state.space],
                            &output,
                            1.0,
                        ) {
                            Ok(elements) => elements,
                            Err(error) => {
                                eprintln!("winit render element generation failed: {error}");
                                state.request_redraw();
                                return;
                            }
                        },
                    );

                    let mut elements = Vec::<LiveRenderElements<GlesRenderer, _>>::with_capacity(
                        background_elements.as_ref().map_or(0, Vec::len)
                            + space_elements.as_ref().map_or(0, Vec::len)
                            + overlay_elements.as_ref().map_or(0, Vec::len),
                    );
                    // Lua config stores the stack bottom-to-top, but Smithay consumes render
                    // elements front-to-back.
                    for layer in render_stack_front_to_back(state) {
                        match layer {
                            DrawLayer::Background => elements.extend(
                                background_elements
                                    .take()
                                    .into_iter()
                                    .flatten()
                                    .map(LiveRenderElements::Custom),
                            ),
                            DrawLayer::Windows => elements.extend(
                                space_elements
                                    .take()
                                    .into_iter()
                                    .flatten()
                                    .map(LiveRenderElements::Space),
                            ),
                            DrawLayer::Overlay => elements.extend(
                                overlay_elements
                                    .take()
                                    .into_iter()
                                    .flatten()
                                    .map(LiveRenderElements::Custom),
                            ),
                            DrawLayer::Cursor => {}
                        }
                    }

                    if let Err(error) = damage_tracker.render_output(
                        renderer,
                        &mut framebuffer,
                        0,
                        &elements,
                        [0.08, 0.05, 0.12, 1.0],
                    ) {
                        eprintln!("winit damage render failed: {error}");
                        state.request_redraw();
                        return;
                    }

                    if let Some(path) = state.pending_screenshot_path.take() {
                        let region = Rectangle::<i32, smithay::utils::Buffer>::from_size(
                            (size.w, size.h).into(),
                        );
                        match renderer.copy_framebuffer(
                            &framebuffer,
                            region,
                            smithay::backend::allocator::Fourcc::Abgr8888,
                        ) {
                            Ok(mapping) => match renderer.map_texture(&mapping) {
                                Ok(bytes) => {
                                    if let Err(error) =
                                        write_ppm_screenshot(&path, size, bytes, mapping.flipped())
                                    {
                                        eprintln!(
                                            "failed to write screenshot {}: {error}",
                                            path.display()
                                        );
                                        state.emit_event(
                                            "screenshot_failed",
                                            serde_json::json!({
                                                "path": path.display().to_string(),
                                                "error": error.to_string(),
                                            }),
                                        );
                                    } else {
                                        println!("wrote screenshot to {}", path.display());
                                        state.emit_event(
                                            "screenshot_written",
                                            serde_json::json!({
                                                "path": path.display().to_string(),
                                                "width": size.w,
                                                "height": size.h,
                                            }),
                                        );
                                    }
                                }
                                Err(error) => {
                                    eprintln!(
                                        "failed to map screenshot framebuffer {}: {error}",
                                        path.display()
                                    );
                                    state.emit_event(
                                        "screenshot_failed",
                                        serde_json::json!({
                                            "path": path.display().to_string(),
                                            "error": format!("map framebuffer: {error}"),
                                        }),
                                    );
                                }
                            },
                            Err(error) => {
                                eprintln!(
                                    "failed to copy screenshot framebuffer {}: {error}",
                                    path.display()
                                );
                                state.emit_event(
                                    "screenshot_failed",
                                    serde_json::json!({
                                        "path": path.display().to_string(),
                                        "error": format!("copy framebuffer: {error}"),
                                    }),
                                );
                            }
                        }
                    }
                }
                if let Err(error) = backend.submit(Some(&[damage])) {
                    eprintln!("winit backend submit failed: {error}");
                    state.request_redraw();
                    return;
                }

                state.space.elements().for_each(|window| {
                    window.send_frame(
                        &output,
                        state.start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    )
                });
                state.space.refresh();
                state.cleanup_window_bookkeeping();
                state.popups.cleanup();
                let _ = state.display_handle.flush_clients();
                backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => {}
        })?;

    println!("evilwm ipc socket at {}", state.ipc_socket_path.display());
    println!(
        "evilwm nested compositor running on WAYLAND_DISPLAY={}",
        state.socket_name.to_string_lossy()
    );
    if let Some(path) = options.config_path.as_deref() {
        println!("loaded config: {}", path.display());
    }
    state.emit_event(
        "startup",
        serde_json::json!({
            "backend": "winit",
            "wayland_display": state.socket_name.to_string_lossy(),
            "ipc_socket": state.ipc_socket_path.display().to_string(),
            "config_path": options
                .config_path
                .as_ref()
                .map(|path| path.display().to_string()),
        }),
    );
    publish_wayland_display(&state.socket_name);
    for command in startup_commands {
        println!("spawning client: {command}");
        spawn_client(&command, &state.socket_name, &state.ipc_socket_path);
    }

    event_loop.run(None, &mut state, move |_| {})?;
    Ok(())
}

pub(super) fn startup_commands(options: &RuntimeOptions) -> Vec<String> {
    let mut commands = options
        .config
        .as_ref()
        .map(|config| config.autostart.clone())
        .unwrap_or_default();

    if let Some(command) = options.command.as_ref() {
        commands.push(command.clone());
    }

    commands
}

pub(super) fn publish_wayland_display(socket_name: &std::ffi::OsStr) {
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", socket_name);
    }
}

pub(super) fn spawn_client(
    command: &str,
    wayland_display: &std::ffi::OsStr,
    ipc_socket_path: &std::path::Path,
) {
    let Some(mut cmd) = build_spawn_command(command, wayland_display, ipc_socket_path) else {
        return;
    };
    let _ = cmd.spawn();
}

/// Build a shell-backed spawn command for config/autostart strings.
///
/// This intentionally executes through `sh -c` instead of direct `exec` so
/// config authors can use shell syntax, environment expansion, and simple
/// pipelines in trusted user-owned config files.
pub(super) fn build_spawn_command(
    command: &str,
    wayland_display: &std::ffi::OsStr,
    ipc_socket_path: &std::path::Path,
) -> Option<std::process::Command> {
    if command.trim().is_empty() {
        return None;
    }

    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c");
    cmd.arg(command);
    cmd.env("WAYLAND_DISPLAY", wayland_display);
    cmd.env("EVILWM_IPC_SOCKET", ipc_socket_path);
    if let Ok(current_exe) = std::env::current_exe() {
        cmd.env("EVILWM_BIN", current_exe);
    }
    Some(cmd)
}

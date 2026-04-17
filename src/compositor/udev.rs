use std::{
    cell::RefCell,
    collections::HashMap,
    error::Error,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use smithay::{
    backend::{
        allocator::{
            Format, Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode,
            compositor::{DrmCompositor, FrameFlags},
            exporter::gbm::GbmFramebufferExporter,
        },
        egl::{EGLContext, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{ImportDma, ImportMemWl, gles::GlesRenderer},
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent},
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{EventLoop, LoopHandle, RegistrationToken},
        drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc},
        input::{Libinput, TapButtonMap},
        rustix::fs::OFlags,
        wayland_server::{Display, backend::GlobalId},
    },
    utils::DeviceFd,
};

use crate::canvas::{Point, Size};

use super::{
    EvilWm, RuntimeOptions, TtyControlAction, UdevRenderElements, build_cursor_elements,
    build_live_space_elements, publish_wayland_display, solid_elements_from_draw_commands,
    spawn_client, startup_commands,
};

const SUPPORTED_COLOR_FORMATS: &[Fourcc] = &[
    Fourcc::Xrgb8888,
    Fourcc::Xbgr8888,
    Fourcc::Argb8888,
    Fourcc::Abgr8888,
];

type GbmDrmCompositor =
    DrmCompositor<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

struct PublishedOutput {
    output: Output,
    global: GlobalId,
}

struct TrackedSurface {
    compositor: GbmDrmCompositor,
    output: Output,
    frame_pending: bool,
}

struct TrackedDrmDevice {
    path: PathBuf,
    _drm: DrmDevice,
    _gbm: GbmDevice<DrmDeviceFd>,
    renderer: GlesRenderer,
    notifier_token: RegistrationToken,
    outputs: Vec<PublishedOutput>,
    surfaces: HashMap<crtc::Handle, TrackedSurface>,
}

fn total_surface_count(tracked_devices: &HashMap<DrmNode, TrackedDrmDevice>) -> usize {
    tracked_devices
        .values()
        .map(|device| device.surfaces.len())
        .sum()
}

fn configure_libinput_device(device: &mut smithay::reexports::input::Device) {
    println!("libinput device added: {}", device.name());

    let tap_fingers = device.config_tap_finger_count();
    if tap_fingers == 0 {
        return;
    }

    match device.config_tap_set_enabled(true) {
        Ok(()) => {
            println!(
                "enabled tap-to-click for {} ({} finger tap support)",
                device.name(),
                tap_fingers
            );
        }
        Err(error) => {
            eprintln!(
                "failed to enable tap-to-click for {}: {error:?}",
                device.name()
            );
            return;
        }
    }

    match device.config_tap_set_button_map(TapButtonMap::LeftRightMiddle) {
        Ok(()) => {
            println!("using left-right-middle tap map for {}", device.name());
        }
        Err(error) => {
            eprintln!(
                "failed to set tap button map for {}: {error:?}",
                device.name()
            );
        }
    }
}

fn render_tracked_surface(
    state: &mut EvilWm,
    renderer: &mut GlesRenderer,
    surface: &mut TrackedSurface,
    crtc: crtc::Handle,
) -> Result<bool, Box<dyn Error>> {
    state.space.refresh();
    state.cleanup_window_bookkeeping();
    state.popups.cleanup();

    let mut cursor_elements = Some(build_cursor_elements(state, &surface.output));
    let mut background_elements = Some(solid_elements_from_draw_commands(
        state,
        &surface.output,
        "draw_background",
    ));
    let mut overlay_elements = Some({
        let mut elements =
            solid_elements_from_draw_commands(state, &surface.output, "draw_overlay");
        elements.extend(super::lock_overlay_elements(state, &surface.output));
        elements
    });
    let mut space_elements = Some(build_live_space_elements(state, renderer, &surface.output));

    let mut elements = Vec::<UdevRenderElements<GlesRenderer, _>>::with_capacity(
        cursor_elements.as_ref().map_or(0, Vec::len)
            + background_elements.as_ref().map_or(0, Vec::len)
            + overlay_elements.as_ref().map_or(0, Vec::len)
            + space_elements.as_ref().map_or(0, Vec::len),
    );
    // Lua config stores the stack bottom-to-top, but Smithay consumes render
    // elements front-to-back.
    for layer in crate::compositor::render_stack_front_to_back(state) {
        match layer {
            crate::lua::DrawLayer::Background => elements.extend(
                background_elements
                    .take()
                    .into_iter()
                    .flatten()
                    .map(UdevRenderElements::Custom),
            ),
            crate::lua::DrawLayer::Windows => elements.extend(
                space_elements
                    .take()
                    .into_iter()
                    .flatten()
                    .map(UdevRenderElements::Space),
            ),
            crate::lua::DrawLayer::Overlay => elements.extend(
                overlay_elements
                    .take()
                    .into_iter()
                    .flatten()
                    .map(UdevRenderElements::Custom),
            ),
            crate::lua::DrawLayer::Cursor => elements.extend(
                cursor_elements
                    .take()
                    .into_iter()
                    .flatten()
                    .map(UdevRenderElements::Custom),
            ),
        }
    }

    let render_result = surface
        .compositor
        .render_frame::<_, UdevRenderElements<_, _>>(
            renderer,
            &elements,
            [0.08, 0.05, 0.12, 1.0],
            FrameFlags::empty(),
        )?;
    let has_damage = !render_result.is_empty;
    drop(render_result);

    if has_damage {
        surface.compositor.queue_frame(())?;
        surface.frame_pending = true;

        state.space.elements().for_each(|window| {
            window.send_frame(
                &surface.output,
                state.start_time.elapsed(),
                Some(Duration::ZERO),
                |_, _| Some(surface.output.clone()),
            )
        });
    }

    let _ = state.display_handle.flush_clients();
    println!("rendered tty frame for {:?} damage={has_damage}", crtc);
    Ok(has_damage)
}

fn render_devices_if_needed(
    state: &mut EvilWm,
    tracked_devices: &mut HashMap<DrmNode, TrackedDrmDevice>,
) {
    if !state.redraw_requested() {
        return;
    }

    let mut waiting_for_vblank = false;

    for (node, device) in tracked_devices.iter_mut() {
        let renderer = &mut device.renderer;
        for (crtc, surface) in device.surfaces.iter_mut() {
            if surface.frame_pending {
                waiting_for_vblank = true;
                continue;
            }

            match render_tracked_surface(state, renderer, surface, *crtc) {
                Ok(_has_damage) => {
                    if surface.frame_pending {
                        waiting_for_vblank = true;
                    }
                }
                Err(error) => {
                    eprintln!(
                        "failed to render tty frame on {:?} {:?}: {error}",
                        node, crtc
                    );
                }
            }
        }
    }

    if !waiting_for_vblank {
        state.clear_redraw_request();
    }

    let surface_count = total_surface_count(tracked_devices);
    if surface_count == 0 {
        if !state.tty_no_scanout_warned {
            println!("tty backend has outputs published, but no scanout surfaces are active yet");
            state.tty_no_scanout_warned = true;
        }
    } else {
        state.tty_no_scanout_warned = false;
    }
}

fn render_if_needed(
    state: &mut EvilWm,
    tracked_devices: &Rc<RefCell<HashMap<DrmNode, TrackedDrmDevice>>>,
) {
    if !state.tty_session_active {
        return;
    }
    let mut tracked_devices = tracked_devices.borrow_mut();
    render_devices_if_needed(state, &mut tracked_devices);
}

fn reset_tracked_surface_states(tracked_devices: &Rc<RefCell<HashMap<DrmNode, TrackedDrmDevice>>>) {
    for (node, device) in tracked_devices.borrow_mut().iter_mut() {
        for (crtc, surface) in device.surfaces.iter_mut() {
            if let Err(error) = surface.compositor.reset_state() {
                eprintln!(
                    "failed to reset DRM compositor state on {:?} {:?}: {error}",
                    node, crtc
                );
            } else {
                surface.frame_pending = false;
                println!("reset DRM compositor state on {:?} {:?}", node, crtc);
            }
        }
    }
}

fn remove_published_outputs(
    loop_handle: &LoopHandle<'_, EvilWm>,
    state: &mut EvilWm,
    tracked_devices: &Rc<RefCell<HashMap<DrmNode, TrackedDrmDevice>>>,
    node: DrmNode,
) {
    if let Some(device) = tracked_devices.borrow_mut().remove(&node) {
        loop_handle.remove(device.notifier_token);
        for published in device.outputs {
            state.remove_output_state(&published.output.name());
            state.space.unmap_output(&published.output);
            state
                .display_handle
                .remove_global::<EvilWm>(published.global);
        }
        state.request_redraw();
        state.tty_no_scanout_warned = false;
        state.sync_output_positions_to_viewport();
        state.sync_primary_output_state_from_space();
        state.notify_output_management_state();
        if state.space.outputs().next().is_some() {
            state.center_pointer_on_primary_output();
        }
    }
}

fn publish_drm_outputs(
    loop_handle: &LoopHandle<'_, EvilWm>,
    state: &mut EvilWm,
    session: &Rc<RefCell<LibSeatSession>>,
    tracked_devices: &Rc<RefCell<HashMap<DrmNode, TrackedDrmDevice>>>,
    known_paths: &Rc<RefCell<HashMap<DrmNode, PathBuf>>>,
    node: DrmNode,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    remove_published_outputs(loop_handle, state, tracked_devices, node);

    let fd = session.borrow_mut().open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
    )?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (mut drm, notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd.clone())?;
    let egl_display = unsafe { EGLDisplay::new(gbm.clone()) }?;
    let egl_context = EGLContext::new(&egl_display)?;
    let renderer = unsafe { GlesRenderer::new(egl_context) }?;
    state.shm_state.update_formats(renderer.shm_formats());
    let render_formats = ImportDma::dmabuf_formats(&renderer)
        .iter()
        .copied()
        .collect::<Vec<Format>>();

    let tracked_for_notifier = tracked_devices.clone();
    let notifier_token =
        loop_handle.insert_source(notifier, move |event, _, state| match event {
            DrmEvent::VBlank(crtc) => {
                println!("drm vblank on {:?} {:?}", node, crtc);
                let mut tracked_devices = tracked_for_notifier.borrow_mut();
                let Some(device) = tracked_devices.get_mut(&node) else {
                    return;
                };
                let Some(surface) = device.surfaces.get_mut(&crtc) else {
                    return;
                };
                if let Err(error) = surface.compositor.frame_submitted() {
                    eprintln!("frame_submitted error on {:?} {:?}: {error}", node, crtc);
                }
                surface.frame_pending = false;
                render_devices_if_needed(state, &mut tracked_devices);
            }
            DrmEvent::Error(error) => eprintln!("drm event error on {:?}: {:?}", node, error),
        })?;

    let resources = fd.resource_handles()?;
    let mut outputs = Vec::new();
    let mut surfaces = HashMap::new();
    let mut used_crtcs = Vec::<crtc::Handle>::new();
    let mut had_any = false;

    for connector_handle in resources.connectors() {
        let info = match drm.get_connector(*connector_handle, true) {
            Ok(info) => info,
            Err(error) => {
                eprintln!(
                    "failed to query connector {:?} on {}: {error}",
                    connector_handle,
                    path.display()
                );
                continue;
            }
        };

        if info.state() != connector::State::Connected || info.modes().is_empty() {
            continue;
        }
        had_any = true;

        let output_name = format!("{}-{}", info.interface().as_str(), info.interface_id());
        let mode = info
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .copied()
            .unwrap_or_else(|| info.modes()[0]);
        let wl_mode = OutputMode::from(mode);
        let (phys_w, phys_h) = info.size().unwrap_or((0, 0));
        let output = Output::new(
            output_name.clone(),
            PhysicalProperties {
                size: (phys_w as i32, phys_h as i32).into(),
                subpixel: Subpixel::from(info.subpixel()),
                make: "unknown".into(),
                model: path.display().to_string(),
            },
        );
        let global = output.create_global::<EvilWm>(&state.display_handle);

        let x = state
            .space
            .outputs()
            .filter_map(|mapped| {
                state
                    .space
                    .output_geometry(mapped)
                    .map(|geometry| geometry.size.w)
            })
            .sum::<i32>();
        output.set_preferred(wl_mode);
        output.change_current_state(Some(wl_mode), None, None, Some((x, 0).into()));
        state.space.map_output(&output, (x, 0));

        state.sync_output_state(
            &output_name,
            Point::new(x as f64, 0.0),
            Size::new(wl_mode.size.w as f64, wl_mode.size.h as f64),
        );
        if state.space.outputs().count() == 1
            && let Some(primary) = state.output_state_for_name(&output_name).cloned()
        {
            state.output_state = primary;
        }
        state.notify_output_management_state();

        let chosen_crtc = info
            .current_encoder()
            .and_then(|encoder| fd.get_encoder(encoder).ok())
            .and_then(|encoder| {
                fd.resource_handles()
                    .ok()?
                    .filter_crtcs(encoder.possible_crtcs())
                    .into_iter()
                    .find(|crtc| !used_crtcs.contains(crtc))
            })
            .or_else(|| {
                info.encoders()
                    .iter()
                    .filter_map(|encoder| fd.get_encoder(*encoder).ok())
                    .flat_map(|encoder| {
                        fd.resource_handles()
                            .ok()
                            .map(|resources| resources.filter_crtcs(encoder.possible_crtcs()))
                            .into_iter()
                            .flatten()
                    })
                    .find(|crtc| !used_crtcs.contains(crtc))
            });

        if let Some(crtc) = chosen_crtc {
            match drm.create_surface(crtc, mode, &[*connector_handle]) {
                Ok(surface_handle) => {
                    let allocator = GbmAllocator::new(
                        gbm.clone(),
                        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
                    );
                    match DrmCompositor::new(
                        &output,
                        surface_handle,
                        None,
                        allocator,
                        GbmFramebufferExporter::new(gbm.clone(), None),
                        SUPPORTED_COLOR_FORMATS.iter().copied(),
                        render_formats.iter().copied(),
                        drm.cursor_size(),
                        Some(gbm.clone()),
                    ) {
                        Ok(compositor) => {
                            println!(
                                "created drm compositor for connector {:?} on {:?}",
                                connector_handle, crtc
                            );
                            used_crtcs.push(crtc);
                            surfaces.insert(
                                crtc,
                                TrackedSurface {
                                    compositor,
                                    output: output.clone(),
                                    frame_pending: false,
                                },
                            );
                        }
                        Err(error) => {
                            eprintln!(
                                "failed to create drm compositor for connector {:?} on {:?}: {error:?}",
                                connector_handle, crtc
                            );
                        }
                    }
                }
                Err(error) => {
                    eprintln!(
                        "failed to create drm surface for connector {:?} on {:?}: {error}",
                        connector_handle, crtc
                    );
                }
            }
        } else {
            eprintln!(
                "no compatible free CRTC found for connector {:?} on {}",
                connector_handle,
                path.display()
            );
        }

        outputs.push(PublishedOutput { output, global });
    }

    if had_any {
        println!(
            "published {} connected output(s) for DRM node {:?}",
            outputs.len(),
            node
        );
        if surfaces.is_empty() {
            eprintln!(
                "DRM node {:?} has connected outputs but no active scanout surfaces yet",
                node
            );
        }
    }

    tracked_devices.borrow_mut().insert(
        node,
        TrackedDrmDevice {
            path: path.to_path_buf(),
            _drm: drm,
            _gbm: gbm,
            renderer,
            notifier_token,
            outputs,
            surfaces,
        },
    );
    known_paths.borrow_mut().insert(node, path.to_path_buf());

    state.request_redraw();
    state.tty_no_scanout_warned = false;
    state.sync_output_positions_to_viewport();
    state.sync_primary_output_state_from_space();
    state.notify_output_management_state();
    if state.window_models.is_empty() {
        state.center_pointer_on_primary_output();
    }
    Ok(())
}

pub fn run_udev(options: RuntimeOptions) -> Result<(), Box<dyn Error>> {
    let startup_commands = startup_commands(&options);
    let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();
    let display: Display<EvilWm> = Display::new()?;
    let mut state = EvilWm::new(
        &mut event_loop,
        display,
        options.config_path.clone(),
        options.config.clone(),
    )?;

    let (session, notifier) = LibSeatSession::new()?;
    let seat_name = session.seat().to_string();
    let session = Rc::new(RefCell::new(session));
    let tty_loop_signal = event_loop.get_signal();
    let tty_session = session.clone();
    state.tty_control = Some(Rc::new(RefCell::new(Box::new(
        move |action| match action {
            TtyControlAction::Quit => tty_loop_signal.stop(),
            TtyControlAction::SwitchVt(vt) => {
                if let Err(error) = tty_session.borrow_mut().change_vt(vt) {
                    eprintln!("failed to switch to vt{vt}: {error}");
                }
            }
        },
    ))));

    let udev_backend = UdevBackend::new(&seat_name)?;
    let initial_devices = udev_backend
        .device_list()
        .filter_map(|(device_id, path)| {
            Some((DrmNode::from_dev_id(device_id).ok()?, path.to_path_buf()))
        })
        .collect::<Vec<_>>();
    let initial_device_count = initial_devices.len();

    let mut libinput_context = Libinput::new_with_udev::<
        LibinputSessionInterface<Rc<RefCell<LibSeatSession>>>,
    >(session.clone().into());
    libinput_context
        .udev_assign_seat(&seat_name)
        .map_err(|_| format!("failed to assign libinput to seat {seat_name}"))?;
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    event_loop
        .handle()
        .insert_source(libinput_backend, move |mut event, _, state| {
            match &mut event {
                InputEvent::DeviceAdded { device } => {
                    configure_libinput_device(device);
                }
                InputEvent::DeviceRemoved { device } => {
                    println!("libinput device removed: {}", device.name());
                }
                _ => {}
            }

            state.process_input_event(event);
        })?;

    let tracked_devices = Rc::new(RefCell::new(HashMap::<DrmNode, TrackedDrmDevice>::new()));
    let known_paths = Rc::new(RefCell::new(HashMap::<DrmNode, PathBuf>::new()));
    let tracked_for_session = tracked_devices.clone();
    let mut libinput_for_session = libinput_context.clone();
    event_loop
        .handle()
        .insert_source(notifier, move |event, &mut (), state| match event {
            SessionEvent::PauseSession => {
                println!("pausing tty session");
                state.emit_event("tty_session_paused", serde_json::json!({}));
                state.tty_session_active = false;
                libinput_for_session.suspend();
                for device in tracked_for_session.borrow_mut().values_mut() {
                    for surface in device.surfaces.values_mut() {
                        surface.frame_pending = false;
                    }
                }
            }
            SessionEvent::ActivateSession => {
                println!("activating tty session");
                if let Err(error) = libinput_for_session.resume() {
                    state.emit_event(
                        "tty_session_activation_failed",
                        serde_json::json!({ "error": format!("{error:?}") }),
                    );
                    eprintln!("failed to resume libinput context: {error:?}");
                    state.loop_signal.stop();
                    return;
                }

                state.emit_event("tty_session_activated", serde_json::json!({}));
                state.tty_session_active = true;
                reset_tracked_surface_states(&tracked_for_session);
                state.request_redraw();
                render_if_needed(state, &tracked_for_session);
            }
        })?;

    for (node, path) in &initial_devices {
        if let Err(error) = publish_drm_outputs(
            &loop_handle,
            &mut state,
            &session,
            &tracked_devices,
            &known_paths,
            *node,
            path,
        ) {
            eprintln!(
                "failed to inspect initial DRM node {:?} ({}): {error}",
                node,
                path.display()
            );
        }
    }

    let tracked_for_udev = tracked_devices.clone();
    let known_paths_for_udev = known_paths.clone();
    let session_for_udev = session.clone();
    let loop_handle_for_udev = loop_handle.clone();
    event_loop
        .handle()
        .insert_source(udev_backend, move |event, _, state| match event {
            UdevEvent::Added { device_id, path } => {
                println!("udev drm device added: {:?} {:?}", device_id, path);
                if let Ok(node) = DrmNode::from_dev_id(device_id)
                    && let Err(error) = publish_drm_outputs(
                        &loop_handle_for_udev,
                        state,
                        &session_for_udev,
                        &tracked_for_udev,
                        &known_paths_for_udev,
                        node,
                        &path,
                    )
                {
                    eprintln!("failed to inspect DRM node {:?}: {error}", node);
                }
            }
            UdevEvent::Changed { device_id } => {
                println!("udev drm device changed: {:?}", device_id);
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    let path = tracked_for_udev
                        .borrow()
                        .get(&node)
                        .map(|device| device.path.clone())
                        .or_else(|| known_paths_for_udev.borrow().get(&node).cloned());
                    if let Some(path) = path
                        && let Err(error) = publish_drm_outputs(
                            &loop_handle_for_udev,
                            state,
                            &session_for_udev,
                            &tracked_for_udev,
                            &known_paths_for_udev,
                            node,
                            &path,
                        )
                    {
                        eprintln!("failed to refresh DRM node {:?}: {error}", node);
                    }
                }
            }
            UdevEvent::Removed { device_id } => {
                println!("udev drm device removed: {:?}", device_id);
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    known_paths_for_udev.borrow_mut().remove(&node);
                    remove_published_outputs(&loop_handle_for_udev, state, &tracked_for_udev, node);
                }
            }
        })?;

    #[cfg(feature = "xwayland")]
    super::spawn_xwayland(&state.display_handle, &event_loop.handle());

    println!("evilwm ipc socket at {}", state.ipc_socket_path.display());
    println!(
        "evilwm udev renderer running on seat={} WAYLAND_DISPLAY={} drm_devices={}",
        seat_name,
        state.socket_name.to_string_lossy(),
        initial_device_count
    );
    state.emit_event(
        "tty_backend_started",
        serde_json::json!({
            "seat": seat_name,
            "wayland_display": state.socket_name.to_string_lossy(),
            "drm_devices": initial_device_count,
            "config_path": options
                .config_path
                .as_ref()
                .map(|path| path.display().to_string()),
        }),
    );
    if let Some(path) = options.config_path.as_deref() {
        println!("loaded config: {}", path.display());
    }
    println!(
        "tty startup note: use a spare VT, keep this console output for diagnostics, and prefer the launcher script for the supported flow"
    );
    if initial_device_count == 0 {
        println!("warning: no DRM devices detected by udev yet");
    }

    publish_wayland_display(&state.socket_name);
    for command in startup_commands {
        println!("spawning client: {command}");
        spawn_client(&command, &state.socket_name, &state.ipc_socket_path);
    }

    let tracked_for_run = tracked_devices.clone();
    event_loop.run(None, &mut state, move |state| {
        render_if_needed(state, &tracked_for_run);
        state.space.refresh();
        state.cleanup_window_bookkeeping();
        state.popups.cleanup();
        let _ = state.display_handle.flush_clients();
    })?;

    println!("shutting down tty backend");
    state.emit_event("tty_backend_shutdown", serde_json::json!({}));
    libinput_context.suspend();
    let nodes = tracked_devices.borrow().keys().copied().collect::<Vec<_>>();
    for node in nodes {
        remove_published_outputs(&loop_handle, &mut state, &tracked_devices, node);
    }

    Ok(())
}

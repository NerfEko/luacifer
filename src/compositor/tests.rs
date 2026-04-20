use super::{
    ActiveInteractiveKind, ActiveInteractiveOp, apply_trackpad_swipe_to_viewport,
    build_spawn_command, compile_window_rules, pinch_relative_factor, render_stack_front_to_back,
    resize_edges_from,
};
#[cfg(feature = "udev")]
use super::{TtyControlAction, tty_control_action_for};
#[cfg(feature = "lua")]
use super::{format_live_hook_error, startup_commands};
#[cfg(any(feature = "lua", feature = "udev"))]
use crate::canvas::Point as CanvasPoint;
use crate::canvas::{Size, Viewport};
#[cfg(feature = "lua")]
use crate::compositor::EvilWm;
#[cfg(feature = "lua")]
use crate::compositor::{HeadlessOptions, run_headless};
#[cfg(feature = "lua")]
use crate::input::{BindingMap, ModifierSet};
#[cfg(feature = "lua")]
use crate::lua::{
    BindingConfig, CanvasConfig, Config, ConfigError, DrawConfig, DrawLayer, LiveLuaHooks,
    LuaSession, PropertyValue, ResolveFocusRequest,
};
#[cfg(feature = "lua")]
use crate::window::{Window, WindowRule};
use crate::{canvas::Point, window::WindowId};
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
#[cfg(any(feature = "lua", feature = "udev"))]
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use smithay::utils::Transform;
use std::ffi::OsStr;

#[cfg(feature = "lua")]
fn create_live_test_state(config: Option<Config>) -> EvilWm {
    let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
    let display: Display<EvilWm> = Display::new().expect("display");
    EvilWm::new(&mut event_loop, display, None, config).expect("state")
}

#[cfg(feature = "lua")]
#[test]
fn canvas_input_navigation_flags_default_true_and_can_be_disabled() {
    let default_state = create_live_test_state(None);
    assert!(default_state.canvas_allows_pointer_zoom());
    assert!(default_state.canvas_allows_middle_click_pan());
    assert!(default_state.canvas_allows_gesture_navigation());

    let config = Config {
        backend: Some("winit".into()),
        canvas: CanvasConfig {
            allow_pointer_zoom: false,
            allow_middle_click_pan: false,
            allow_gesture_navigation: false,
            ..CanvasConfig::default()
        },
        draw: DrawConfig::default(),
        window: crate::lua::WindowConfig::default(),
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig::default(),
        autostart: Vec::new(),
        bindings: Vec::new(),
        rules: Vec::new(),
        source_root: std::path::PathBuf::from("."),
    };
    let disabled_state = create_live_test_state(Some(config));
    assert!(!disabled_state.canvas_allows_pointer_zoom());
    assert!(!disabled_state.canvas_allows_middle_click_pan());
    assert!(!disabled_state.canvas_allows_gesture_navigation());
}

#[cfg(feature = "lua")]
#[test]
fn remembered_size_wins_over_client_preferred_size() {
    let config = Config {
        backend: Some("winit".into()),
        canvas: CanvasConfig::default(),
        draw: DrawConfig::default(),
        window: crate::lua::WindowConfig {
            use_client_default_size: true,
            remember_sizes_by_app_id: true,
            hide_client_decorations: false,
        },
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig::default(),
        autostart: Vec::new(),
        bindings: Vec::new(),
        rules: Vec::new(),
        source_root: std::path::PathBuf::from("."),
    };
    let mut state = create_live_test_state(Some(config));
    state
        .remembered_app_sizes
        .insert("xterm".into(), Size::new(900.0, 700.0));

    let properties = crate::window::WindowProperties {
        app_id: Some("xterm".into()),
        title: Some("xterm".into()),
        pid: None,
    };
    let chosen =
        state.requested_initial_window_size_from(&properties, Some(Size::new(640.0, 480.0)));
    assert_eq!(chosen, Some(Size::new(900.0, 700.0)));
}

#[cfg(feature = "lua")]
#[test]
fn client_preferred_size_is_used_when_no_remembered_size_exists() {
    let config = Config {
        backend: Some("winit".into()),
        canvas: CanvasConfig::default(),
        draw: DrawConfig::default(),
        window: crate::lua::WindowConfig {
            use_client_default_size: true,
            remember_sizes_by_app_id: true,
            hide_client_decorations: false,
        },
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig::default(),
        autostart: Vec::new(),
        bindings: Vec::new(),
        rules: Vec::new(),
        source_root: std::path::PathBuf::from("."),
    };
    let state = create_live_test_state(Some(config));
    let properties = crate::window::WindowProperties {
        app_id: Some("xterm".into()),
        title: Some("xterm".into()),
        pid: None,
    };
    let chosen =
        state.requested_initial_window_size_from(&properties, Some(Size::new(640.0, 480.0)));
    assert_eq!(chosen, Some(Size::new(640.0, 480.0)));
}

#[cfg(feature = "lua")]
#[test]
fn hide_client_decorations_prefers_server_side_mode() {
    let mut config = Config {
        backend: Some("winit".into()),
        canvas: CanvasConfig::default(),
        draw: DrawConfig::default(),
        window: crate::lua::WindowConfig::default(),
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig::default(),
        autostart: Vec::new(),
        bindings: Vec::new(),
        rules: Vec::new(),
        source_root: std::path::PathBuf::from("."),
    };
    config.window.hide_client_decorations = true;
    let state = create_live_test_state(Some(config));
    assert_eq!(
            state.preferred_decoration_mode(),
            smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode::ServerSide
        );
}

#[cfg(feature = "lua")]
#[test]
fn client_decorations_remain_enabled_by_default() {
    let state = create_live_test_state(None);
    assert_eq!(
            state.preferred_decoration_mode(),
            smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode::ClientSide
        );
}

#[cfg(feature = "lua")]
fn create_test_output(name: &str, loc: (i32, i32), size: (i32, i32)) -> Output {
    let output = Output::new(
        name.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "evilwm-test".into(),
            model: "test".into(),
        },
    );
    let mode = OutputMode {
        size: size.into(),
        refresh: 60_000,
    };
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some(loc.into()));
    output.set_preferred(mode);
    output
}

#[cfg(feature = "lua")]
#[test]
fn multi_output_state_snapshot_enumerates_real_outputs() {
    let mut state = create_live_test_state(None);
    let left = create_test_output("left", (0, 0), (1280, 720));
    let right = create_test_output("right", (1280, 0), (1920, 1080));
    state.space.map_output(&left, (0, 0));
    state.space.map_output(&right, (1280, 0));
    state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(1280.0, 720.0));
    state.register_output_state(
        "right",
        CanvasPoint::new(1280.0, 0.0),
        Size::new(1920.0, 1080.0),
    );

    let snapshot = state.state_snapshot();
    assert_eq!(snapshot.outputs.len(), 2);
    assert_eq!(snapshot.outputs[0].id, "left");
    assert_eq!(snapshot.outputs[1].id, "right");
    assert_eq!(snapshot.outputs[0].logical_x, 0.0);
    assert_eq!(snapshot.outputs[1].logical_x, 1280.0);
}

#[cfg(feature = "lua")]
#[test]
fn multi_output_visible_world_uses_per_output_viewports() {
    let mut state = create_live_test_state(None);
    let left = create_test_output("left", (0, 0), (800, 600));
    let right = create_test_output("right", (800, 0), (1600, 900));
    state.space.map_output(&left, (0, 0));
    state.space.map_output(&right, (800, 0));
    state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
    state.register_output_state(
        "right",
        CanvasPoint::new(800.0, 0.0),
        Size::new(1600.0, 900.0),
    );

    {
        let left_state = state
            .output_state_for_name_mut("left")
            .expect("left output state");
        left_state
            .viewport_mut()
            .pan_world(crate::canvas::Vec2::new(100.0, 50.0));
    }
    {
        let right_state = state
            .output_state_for_name_mut("right")
            .expect("right output state");
        right_state
            .viewport_mut()
            .pan_world(crate::canvas::Vec2::new(-20.0, 10.0));
        right_state
            .viewport_mut()
            .zoom_at_screen(CanvasPoint::new(0.0, 0.0), 2.0);
    }

    let snapshot = state.state_snapshot();
    let left_output = snapshot
        .outputs
        .iter()
        .find(|output| output.id == "left")
        .expect("left snapshot");
    let right_output = snapshot
        .outputs
        .iter()
        .find(|output| output.id == "right")
        .expect("right snapshot");

    assert_eq!(
        left_output.viewport.visible_world,
        crate::canvas::Rect::new(100.0, 50.0, 800.0, 600.0)
    );
    assert_eq!(
        right_output.viewport.visible_world,
        crate::canvas::Rect::new(-20.0, 10.0, 800.0, 450.0)
    );
    assert_ne!(
        left_output.viewport.visible_world,
        right_output.viewport.visible_world
    );
}

#[cfg(all(feature = "lua", feature = "udev"))]
#[test]
fn tty_screen_pointer_positions_map_into_world_space() {
    let mut state = create_live_test_state(None);
    state.tty_control = Some(std::rc::Rc::new(std::cell::RefCell::new(Box::new(|_| {}))));

    let output = create_test_output("tty", (0, 0), (800, 600));
    state.space.map_output(&output, (0, 0));
    state.register_output_state("tty", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
    let output_state = state
        .output_state_for_name_mut("tty")
        .expect("output state");
    output_state
        .viewport_mut()
        .pan_world(crate::canvas::Vec2::new(100.0, 50.0));
    output_state
        .viewport_mut()
        .zoom_at_screen(CanvasPoint::new(0.0, 0.0), 2.0);

    let mapped = state.screen_to_world_pointer_position((200.0, 100.0).into());
    assert_eq!(mapped, (200.0, 100.0).into());
}

#[cfg(all(feature = "lua", feature = "udev"))]
#[test]
fn tty_cursor_uses_world_pointer_position() {
    let mut state = create_live_test_state(None);
    state.tty_control = Some(std::rc::Rc::new(std::cell::RefCell::new(Box::new(|_| {}))));

    let output = create_test_output("tty", (0, 0), (800, 600));
    state.space.map_output(&output, (0, 0));
    state.register_output_state("tty", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
    let output_state = state
        .output_state_for_name_mut("tty")
        .expect("output state");
    output_state
        .viewport_mut()
        .pan_world(crate::canvas::Vec2::new(100.0, 50.0));
    output_state
        .viewport_mut()
        .zoom_at_screen(CanvasPoint::new(0.0, 0.0), 2.0);

    state
        .seat
        .get_pointer()
        .expect("pointer")
        .set_location((200.0, 100.0).into());
    let elements = super::build_cursor_elements(&state, &output);
    assert!(!elements.is_empty());
}

#[cfg(all(feature = "lua", feature = "udev"))]
#[test]
fn tty_pan_keeps_cursor_screen_position_stable() {
    let mut state = create_live_test_state(None);
    state.tty_control = Some(std::rc::Rc::new(std::cell::RefCell::new(Box::new(|_| {}))));

    let output = create_test_output("tty", (0, 0), (800, 600));
    state.space.map_output(&output, (0, 0));
    state.register_output_state("tty", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
    state
        .seat
        .get_pointer()
        .expect("pointer")
        .set_location((200.0, 100.0).into());

    let before_world = state
        .seat
        .get_pointer()
        .expect("pointer")
        .current_location();
    let before_screen = state
        .output_state_for_output(&output)
        .expect("output state")
        .viewport()
        .world_to_screen(CanvasPoint::new(before_world.x, before_world.y));

    state.pan_all_viewports(crate::canvas::Vec2::new(30.0, -20.0));

    let after_world = state
        .seat
        .get_pointer()
        .expect("pointer")
        .current_location();
    let after_screen = state
        .output_state_for_output(&output)
        .expect("output state")
        .viewport()
        .world_to_screen(CanvasPoint::new(after_world.x, after_world.y));

    assert_eq!(after_world, (230.0, 80.0).into());
    assert_eq!(before_screen, after_screen);
}

#[test]
fn interactive_op_reports_incremental_and_total_delta() {
    let mut op = ActiveInteractiveOp::new(
        ActiveInteractiveKind::Move,
        WindowId(7),
        Point::new(100.0, 200.0),
        None,
        Some(272),
    );

    let first = op.advance(Point::new(130.0, 250.0));
    assert_eq!(first.x, 30.0);
    assert_eq!(first.y, 50.0);

    let second = op.advance(Point::new(150.0, 255.0));
    assert_eq!(second.x, 20.0);
    assert_eq!(second.y, 5.0);

    let total = op.total_delta();
    assert_eq!(total.x, 50.0);
    assert_eq!(total.y, 55.0);
}

#[test]
fn interactive_op_only_ends_on_initiating_button() {
    let op = ActiveInteractiveOp::new(
        ActiveInteractiveKind::Resize,
        WindowId(9),
        Point::new(0.0, 0.0),
        Some(crate::window::ResizeEdges::all()),
        Some(274),
    );
    assert!(!op.should_end_on_button(272));
    assert!(op.should_end_on_button(274));

    let op_without_button = ActiveInteractiveOp::new(
        ActiveInteractiveKind::Move,
        WindowId(10),
        Point::new(0.0, 0.0),
        None,
        None,
    );
    assert!(op_without_button.should_end_on_button(272));
}

#[test]
fn trackpad_swipe_pans_viewport_like_canvas_drag() {
    let mut viewport = Viewport::new(Size::new(800.0, 600.0));
    apply_trackpad_swipe_to_viewport(&mut viewport, (30.0, -20.0).into());
    assert_eq!(viewport.world_origin().x, -30.0);
    assert_eq!(viewport.world_origin().y, 20.0);
}

#[test]
fn pinch_relative_factor_uses_incremental_scale() {
    let mut previous = Some(1.0);
    let first = pinch_relative_factor(&mut previous, 1.10).expect("first relative factor");
    let second = pinch_relative_factor(&mut previous, 1.21).expect("second relative factor");

    assert!((first - 1.10).abs() < 1e-9);
    assert!((second - 1.10).abs() < 1e-9);
    assert_eq!(previous, Some(1.21));
}

#[test]
fn build_spawn_command_sets_nested_wayland_display() {
    let cmd = build_spawn_command(
        "foot --server",
        OsStr::new("wayland-99"),
        std::path::Path::new("/tmp/evilwm-ipc.sock"),
    )
    .expect("spawn command");
    let envs = cmd.get_envs().collect::<Vec<_>>();
    assert!(envs.iter().any(|(key, value)| {
        key == &OsStr::new("WAYLAND_DISPLAY") && value == &Some(OsStr::new("wayland-99"))
    }));
    assert!(envs.iter().any(|(key, value)| {
        key == &OsStr::new("EVILWM_IPC_SOCKET")
            && value == &Some(OsStr::new("/tmp/evilwm-ipc.sock"))
    }));
    assert_eq!(cmd.get_program(), OsStr::new("sh"));
    assert_eq!(
        cmd.get_args().collect::<Vec<_>>(),
        vec![OsStr::new("-c"), OsStr::new("foot --server")]
    );
}

#[test]
fn build_spawn_command_preserves_shell_quoted_arguments() {
    let cmd = build_spawn_command(
        "foot --title \"hello world\" --server",
        OsStr::new("wayland-99"),
        std::path::Path::new("/tmp/evilwm-ipc.sock"),
    )
    .expect("spawn command");
    assert_eq!(cmd.get_program(), OsStr::new("sh"));
    assert_eq!(
        cmd.get_args().collect::<Vec<_>>(),
        vec![
            OsStr::new("-c"),
            OsStr::new("foot --title \"hello world\" --server"),
        ]
    );
}

#[test]
fn build_spawn_command_rejects_empty_command() {
    assert!(
        build_spawn_command(
            "   ",
            OsStr::new("wayland-99"),
            std::path::Path::new("/tmp/evilwm-ipc.sock"),
        )
        .is_none()
    );
}

#[cfg(feature = "lua")]
#[test]
fn compile_window_rules_preserves_matchers_and_actions() {
    let config = Config {
        backend: Some("winit".into()),
        canvas: CanvasConfig::default(),
        draw: crate::lua::DrawConfig::default(),
        window: crate::lua::WindowConfig::default(),
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig::default(),
        autostart: Vec::new(),
        bindings: Vec::new(),
        rules: vec![crate::lua::RuleConfig {
            app_id: Some("foot".into()),
            title_contains: Some("scratch".into()),
            floating: Some(false),
            exclude_from_focus: Some(true),
            width: Some(640.0),
            height: Some(480.0),
        }],
        source_root: std::path::PathBuf::from("."),
    };

    assert_eq!(
        compile_window_rules(Some(&config)),
        vec![WindowRule {
            app_id: Some("foot".into()),
            title_contains: Some("scratch".into()),
            floating: Some(false),
            exclude_from_focus: Some(true),
            default_size: Some(Size::new(640.0, 480.0)),
        }]
    );
}

#[cfg(feature = "lua")]
#[test]
fn lua_configured_draw_stack_controls_render_order() {
    let config = Config {
        backend: Some("winit".into()),
        canvas: CanvasConfig::default(),
        draw: DrawConfig {
            stack: vec![
                DrawLayer::Background,
                DrawLayer::Cursor,
                DrawLayer::Windows,
                DrawLayer::WindowOverlay,
                DrawLayer::Popups,
                DrawLayer::Overlay,
            ],
            ..DrawConfig::default()
        },
        window: crate::lua::WindowConfig::default(),
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig::default(),
        autostart: Vec::new(),
        bindings: Vec::new(),
        rules: Vec::new(),
        source_root: std::path::PathBuf::from("."),
    };
    let state = create_live_test_state(Some(config));

    assert_eq!(
        render_stack_front_to_back(&state),
        vec![
            DrawLayer::Overlay,
            DrawLayer::Popups,
            DrawLayer::WindowOverlay,
            DrawLayer::Windows,
            DrawLayer::Cursor,
            DrawLayer::Background,
        ]
    );
}

#[cfg(feature = "lua")]
#[test]
fn configured_draw_clear_color_is_used() {
    let config = Config {
        backend: Some("winit".into()),
        canvas: CanvasConfig::default(),
        draw: DrawConfig {
            clear_color: [0.2, 0.3, 0.4, 1.0],
            ..DrawConfig::default()
        },
        window: crate::lua::WindowConfig::default(),
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig::default(),
        autostart: Vec::new(),
        bindings: Vec::new(),
        rules: Vec::new(),
        source_root: std::path::PathBuf::from("."),
    };
    let state = create_live_test_state(Some(config));
    assert_eq!(state.draw_clear_color(), [0.2, 0.3, 0.4, 1.0]);
}

#[cfg(feature = "lua")]
#[test]
fn resolve_focus_can_start_modifier_drag() {
    let mut state = create_live_test_state(None);
    let id = WindowId(3);
    state.window_models.insert(
        id,
        Window::new(id, crate::canvas::Rect::new(100.0, 120.0, 300.0, 200.0)),
    );
    state.last_pointer_button_pressed = Some(272);
    state
        .seat
        .get_pointer()
        .expect("pointer")
        .set_location((160.0, 170.0).into());

    let hooks = LiveLuaHooks::new(".").expect("live hooks");
    hooks
            .load_script_str(
                r#"
                evil.on.resolve_focus = function(ctx)
                  if ctx.reason == "pointer_button" and ctx.window and ctx.pressed and ctx.modifiers and ctx.modifiers.super and ctx.button == 272 then
                    return {
                      actions = {
                        { kind = "focus_window", id = ctx.window.id },
                        { kind = "begin_move", id = ctx.window.id },
                      },
                    }
                  end
                end
                "#,
                "resolve-drag.lua",
            )
            .expect("load hooks");
    state.live_lua = Some(hooks);

    let snapshot = state.window_snapshot_for_id(id);
    assert!(state.try_live_resolve_focus(ResolveFocusRequest {
        reason: "pointer_button",
        window: snapshot.as_ref(),
        previous: None,
        pointer: Some(CanvasPoint::new(160.0, 170.0)),
        button: Some(272),
        pressed: Some(true),
        modifiers: Some(ModifierSet {
            ctrl: false,
            alt: false,
            shift: false,
            logo: true
        }),
    }));
    assert_eq!(state.focus_stack.focused(), Some(id));
    assert!(matches!(
        state.active_interactive_op.as_ref().map(|op| op.kind),
        Some(ActiveInteractiveKind::Move)
    ));
}

#[cfg(feature = "lua")]
#[test]
fn live_key_hook_runs_before_rust_binding_fallback() {
    let config = Config {
        backend: Some("winit".into()),
        canvas: CanvasConfig::default(),
        draw: crate::lua::DrawConfig::default(),
        window: crate::lua::WindowConfig::default(),
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig::default(),
        autostart: Vec::new(),
        bindings: vec![BindingConfig {
            mods: vec!["Super".into()],
            key: "H".into(),
            action: "pan_left".into(),
            amount: Some(32.0),
            command: None,
        }],
        rules: Vec::new(),
        source_root: std::path::PathBuf::from("."),
    };
    let mut state = create_live_test_state(Some(config.clone()));
    state.bindings = BindingMap::from_config(&config.bindings, 64.0, 1.2);

    let id = WindowId(1);
    state.window_models.insert(
        id,
        Window::new(id, crate::canvas::Rect::new(100.0, 120.0, 300.0, 200.0)),
    );
    state.focus_stack.focus(id);

    let hooks = LiveLuaHooks::new(".").expect("live hooks");
    hooks
        .load_script_str(
            r#"
                evil.on.key = function(ctx)
                  if ctx.bound_action == "pan_left" then
                    evil.window.move(1, 5, 7)
                  end
                end
                "#,
            "live-key.lua",
        )
        .expect("load hooks");
    state.live_lua = Some(hooks);

    assert!(state.handle_resolved_key(
        "H",
        ModifierSet {
            ctrl: false,
            alt: false,
            shift: false,
            logo: true,
        }
    ));

    let window = state.window_models.get(&id).expect("window exists");
    assert_eq!(window.bounds.origin, CanvasPoint::new(5.0, 7.0));
    assert_eq!(state.viewport().world_origin(), CanvasPoint::new(0.0, 0.0));
}

#[cfg(feature = "lua")]
#[test]
fn live_key_hook_can_spawn_via_declarative_action() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("spawned.txt");
    let command = format!("printf 'spawned from hook' > '{}'", marker.display());

    let config = Config {
        backend: Some("winit".into()),
        canvas: CanvasConfig::default(),
        draw: DrawConfig::default(),
        window: crate::lua::WindowConfig::default(),
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig::default(),
        autostart: Vec::new(),
        bindings: vec![BindingConfig {
            mods: vec!["Super".into()],
            key: "Return".into(),
            action: "spawn".into(),
            amount: None,
            command: Some("ignored".into()),
        }],
        rules: Vec::new(),
        source_root: std::path::PathBuf::from("."),
    };
    let mut state = create_live_test_state(Some(config));
    let hooks = LiveLuaHooks::new(".").expect("live hooks");
    hooks
        .load_script_str(
            &format!(
                r#"
                evil.on.key = function(ctx)
                  if ctx.key == "Return" and ctx.modifiers.super then
                    return {{ kind = "spawn", command = "{}" }}
                  end
                end
                "#,
                command.replace('\\', "\\\\").replace('"', "\\\"")
            ),
            "spawn-action.lua",
        )
        .expect("load spawn hook");
    state.live_lua = Some(hooks);

    assert!(state.handle_resolved_key(
        "Return",
        ModifierSet {
            logo: true,
            ..ModifierSet::default()
        }
    ));

    for _ in 0..20 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let contents = std::fs::read_to_string(&marker).expect("spawn hook marker");
    assert_eq!(contents, "spawned from hook");
}

#[cfg(feature = "lua")]
#[test]
fn live_key_without_hook_falls_back_to_rust_binding() {
    let config = Config {
        backend: Some("winit".into()),
        canvas: CanvasConfig::default(),
        draw: crate::lua::DrawConfig::default(),
        window: crate::lua::WindowConfig::default(),
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig::default(),
        autostart: Vec::new(),
        bindings: vec![BindingConfig {
            mods: vec!["Super".into()],
            key: "H".into(),
            action: "pan_left".into(),
            amount: Some(32.0),
            command: None,
        }],
        rules: Vec::new(),
        source_root: std::path::PathBuf::from("."),
    };
    let mut state = create_live_test_state(Some(config.clone()));
    state.bindings = BindingMap::from_config(&config.bindings, 64.0, 1.2);

    assert!(state.handle_resolved_key(
        "H",
        ModifierSet {
            ctrl: false,
            alt: false,
            shift: false,
            logo: true,
        }
    ));
    assert_eq!(
        state.viewport().world_origin(),
        CanvasPoint::new(-32.0, 0.0)
    );
}

#[cfg(feature = "lua")]
#[test]
fn active_interactive_move_helper_advances_and_finishes_sequence() {
    let mut state = create_live_test_state(None);
    let id = WindowId(6);
    state.window_models.insert(
        id,
        Window::new(id, crate::canvas::Rect::new(0.0, 0.0, 300.0, 200.0)),
    );

    let hooks = LiveLuaHooks::new(".").expect("live hooks");
    hooks
        .load_script_str(
            r#"
                evil.on.move_update = function(ctx)
                  evil.window.move(ctx.window.id, 11, 22)
                end
                evil.on.move_end = function(ctx)
                  evil.window.move(ctx.window.id, 33, 44)
                end
                "#,
            "live-move-helper.lua",
        )
        .expect("load move helper hooks");
    state.live_lua = Some(hooks);
    state.active_interactive_op = Some(ActiveInteractiveOp::new(
        ActiveInteractiveKind::Move,
        id,
        CanvasPoint::new(10.0, 10.0),
        None,
        Some(274),
    ));

    state.advance_active_interactive_op(CanvasPoint::new(20.0, 25.0));
    assert_eq!(
        state.window_models.get(&id).expect("window").bounds.origin,
        CanvasPoint::new(11.0, 22.0)
    );

    state.finish_active_interactive_op(272, CanvasPoint::new(20.0, 25.0));
    assert!(
        state.active_interactive_op.is_some(),
        "wrong button must not end move"
    );

    state.finish_active_interactive_op(274, CanvasPoint::new(30.0, 35.0));
    assert_eq!(
        state.window_models.get(&id).expect("window").bounds.origin,
        CanvasPoint::new(33.0, 44.0)
    );
    assert!(
        state.active_interactive_op.is_none(),
        "correct button must end move"
    );
}

#[cfg(feature = "lua")]
#[test]
fn live_move_hook_sequence_updates_window_model() {
    let mut state = create_live_test_state(None);
    let id = WindowId(7);
    state.window_models.insert(
        id,
        Window::new(id, crate::canvas::Rect::new(0.0, 0.0, 300.0, 200.0)),
    );

    let hooks = LiveLuaHooks::new(".").expect("live hooks");
    hooks
        .load_script_str(
            r#"
                evil.on.move_begin = function(ctx)
                  evil.window.move(ctx.window.id, 10, 20)
                end
                evil.on.move_update = function(ctx)
                  evil.window.move(ctx.window.id, 30, 40)
                end
                evil.on.move_end = function(ctx)
                  evil.window.move(ctx.window.id, 50, 60)
                end
                "#,
            "live-move.lua",
        )
        .expect("load move hooks");
    state.live_lua = Some(hooks);

    state.trigger_live_move_begin(id);
    assert_eq!(
        state.window_models.get(&id).expect("window").bounds.origin,
        CanvasPoint::new(10.0, 20.0)
    );
    state.trigger_live_move_update(
        id,
        crate::canvas::Vec2::new(4.0, 6.0),
        CanvasPoint::new(4.0, 6.0),
    );
    assert_eq!(
        state.window_models.get(&id).expect("window").bounds.origin,
        CanvasPoint::new(30.0, 40.0)
    );
    state.trigger_live_move_end(
        id,
        crate::canvas::Vec2::new(9.0, 12.0),
        CanvasPoint::new(9.0, 12.0),
    );
    assert_eq!(
        state.window_models.get(&id).expect("window").bounds.origin,
        CanvasPoint::new(50.0, 60.0)
    );
}

#[cfg(feature = "lua")]
#[test]
fn active_interactive_resize_helper_advances_and_finishes_sequence() {
    let mut state = create_live_test_state(None);
    let id = WindowId(8);
    state.window_models.insert(
        id,
        Window::new(id, crate::canvas::Rect::new(0.0, 0.0, 300.0, 200.0)),
    );

    let hooks = LiveLuaHooks::new(".").expect("live hooks");
    hooks
        .load_script_str(
            r#"
                evil.on.resize_update = function(ctx)
                  evil.window.set_bounds(ctx.window.id, 1, 2, 350, 225)
                end
                evil.on.resize_end = function(ctx)
                  evil.window.set_bounds(ctx.window.id, 3, 4, 400, 260)
                end
                "#,
            "live-resize-helper.lua",
        )
        .expect("load resize helper hooks");
    state.live_lua = Some(hooks);
    let edges = crate::window::ResizeEdges {
        left: false,
        right: true,
        top: false,
        bottom: true,
    };
    state.active_interactive_op = Some(ActiveInteractiveOp::new(
        ActiveInteractiveKind::Resize,
        id,
        CanvasPoint::new(10.0, 10.0),
        Some(edges),
        Some(274),
    ));

    state.advance_active_interactive_op(CanvasPoint::new(25.0, 30.0));
    assert_eq!(
        state.window_models.get(&id).expect("window").bounds,
        crate::canvas::Rect::new(1.0, 2.0, 350.0, 225.0)
    );

    state.finish_active_interactive_op(274, CanvasPoint::new(30.0, 35.0));
    assert_eq!(
        state.window_models.get(&id).expect("window").bounds,
        crate::canvas::Rect::new(3.0, 4.0, 400.0, 260.0)
    );
    assert!(
        state.active_interactive_op.is_none(),
        "correct button must end resize"
    );
}

#[cfg(feature = "lua")]
#[test]
fn live_resize_hook_sequence_updates_window_model() {
    let mut state = create_live_test_state(None);
    let id = WindowId(9);
    state.window_models.insert(
        id,
        Window::new(id, crate::canvas::Rect::new(0.0, 0.0, 300.0, 200.0)),
    );

    let hooks = LiveLuaHooks::new(".").expect("live hooks");
    hooks
        .load_script_str(
            r#"
                evil.on.resize_begin = function(ctx)
                  evil.window.resize(ctx.window.id, 320, 220)
                end
                evil.on.resize_update = function(ctx)
                  evil.window.set_bounds(ctx.window.id, 5, 6, 360, 240)
                end
                evil.on.resize_end = function(ctx)
                  evil.window.set_bounds(ctx.window.id, 7, 8, 400, 260)
                end
                "#,
            "live-resize.lua",
        )
        .expect("load resize hooks");
    state.live_lua = Some(hooks);

    let edges = crate::window::ResizeEdges {
        left: false,
        right: true,
        top: false,
        bottom: true,
    };
    state.trigger_live_resize_begin(id, edges);
    assert_eq!(
        state.window_models.get(&id).expect("window").bounds.size,
        Size::new(320.0, 220.0)
    );
    state.trigger_live_resize_update(
        id,
        crate::canvas::Vec2::new(6.0, 9.0),
        CanvasPoint::new(6.0, 9.0),
        edges,
    );
    assert_eq!(
        state.window_models.get(&id).expect("window").bounds,
        crate::canvas::Rect::new(5.0, 6.0, 360.0, 240.0)
    );
    state.trigger_live_resize_end(
        id,
        crate::canvas::Vec2::new(8.0, 10.0),
        CanvasPoint::new(8.0, 10.0),
        edges,
    );
    assert_eq!(
        state.window_models.get(&id).expect("window").bounds,
        crate::canvas::Rect::new(7.0, 8.0, 400.0, 260.0)
    );
}

#[cfg(feature = "lua")]
#[test]
fn startup_commands_include_config_autostart_and_cli_command() {
    let options = super::RuntimeOptions {
        command: Some("wezterm".into()),
        config_path: None,
        config: Some(Config {
            backend: Some("winit".into()),
            canvas: CanvasConfig::default(),
            draw: crate::lua::DrawConfig::default(),
            window: crate::lua::WindowConfig::default(),
            placement: crate::lua::PlacementConfig::default(),
            tty: crate::lua::TtyConfig::default(),
            autostart: vec!["foot".into(), "waybar".into()],
            bindings: Vec::new(),
            rules: Vec::new(),
            source_root: std::path::PathBuf::from("."),
        }),
    };

    assert_eq!(
        startup_commands(&options),
        vec![
            "foot".to_string(),
            "waybar".to_string(),
            "wezterm".to_string(),
        ]
    );
}

#[cfg(feature = "lua")]
#[test]
fn startup_commands_handle_missing_config_and_command() {
    let options = super::RuntimeOptions {
        command: None,
        config_path: None,
        config: None,
    };

    assert!(startup_commands(&options).is_empty());
}

#[cfg(feature = "udev")]
#[test]
fn tty_control_config_binds_quit_and_vt_switches() {
    let modifiers = ModifierSet {
        ctrl: true,
        alt: true,
        shift: false,
        logo: false,
    };
    let tty = crate::lua::TtyConfig::default();

    assert_eq!(
        tty_control_action_for("BackSpace", modifiers, &tty),
        Some(TtyControlAction::Quit)
    );
    assert_eq!(
        tty_control_action_for("Backspace", modifiers, &tty),
        Some(TtyControlAction::Quit)
    );
    assert_eq!(
        tty_control_action_for("F2", modifiers, &tty),
        Some(TtyControlAction::SwitchVt(2))
    );
    assert_eq!(
        tty_control_action_for("f3", modifiers, &tty),
        Some(TtyControlAction::SwitchVt(3))
    );
    assert_eq!(tty_control_action_for("F13", modifiers, &tty), None);
    assert_eq!(
        tty_control_action_for(
            "F3",
            ModifierSet {
                ctrl: true,
                alt: false,
                shift: false,
                logo: false,
            },
            &tty,
        ),
        None
    );
}

#[cfg(feature = "udev")]
#[test]
fn tty_control_config_allows_custom_quit_and_vt_modifiers() {
    let tty = crate::lua::TtyConfig {
        quit_mods: vec!["Ctrl".into(), "Shift".into()],
        quit_key: "Q".into(),
        vt_switch_modifiers: vec!["Alt".into()],
        output_layout: crate::lua::TtyOutputLayout::Horizontal,
    };

    assert_eq!(
        tty_control_action_for(
            "Q",
            ModifierSet {
                ctrl: true,
                alt: false,
                shift: true,
                logo: false,
            },
            &tty,
        ),
        Some(TtyControlAction::Quit)
    );
    assert_eq!(
        tty_control_action_for(
            "F4",
            ModifierSet {
                ctrl: false,
                alt: true,
                shift: false,
                logo: false,
            },
            &tty,
        ),
        Some(TtyControlAction::SwitchVt(4))
    );
}

#[cfg(feature = "udev")]
#[test]
fn sync_primary_output_state_falls_back_when_no_outputs_exist() {
    let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
    let display: Display<EvilWm> = Display::new().expect("display");
    let mut state = EvilWm::new(&mut event_loop, display, None, None).expect("state");

    state.register_output_state("test", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
    assert!(!state.output_states.is_empty());
    state.sync_primary_output_state_from_space();

    assert!(
        state.output_states.is_empty(),
        "output_states should be cleared when no space outputs exist"
    );
    // viewport() should fall back to the sentinel output_state
    assert_eq!(
        state.viewport().screen_size(),
        state.output_state.viewport().screen_size()
    );
}

#[cfg(feature = "udev")]
#[test]
fn tty_output_layout_can_stack_outputs_vertically() {
    let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
    let display: Display<EvilWm> = Display::new().expect("display");
    let config = Config {
        backend: Some("udev".into()),
        canvas: CanvasConfig::default(),
        draw: DrawConfig::default(),
        window: crate::lua::WindowConfig::default(),
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig {
            output_layout: crate::lua::TtyOutputLayout::Vertical,
            ..crate::lua::TtyConfig::default()
        },
        autostart: Vec::new(),
        bindings: Vec::new(),
        rules: Vec::new(),
        source_root: std::path::PathBuf::from("."),
    };
    let mut state = EvilWm::new(&mut event_loop, display, None, Some(config)).expect("state");
    let top = create_test_output("top", (0, 0), (800, 600));
    let bottom = create_test_output("bottom", (0, 0), (1024, 768));
    state.space.map_output(&top, (0, 0));
    state.space.map_output(&bottom, (0, 0));

    state.sync_output_positions_to_viewport();

    assert_eq!(
        state
            .output_state_for_output(&top)
            .expect("top state")
            .logical_position(),
        CanvasPoint::new(0.0, 0.0)
    );
    assert_eq!(
        state
            .output_state_for_output(&bottom)
            .expect("bottom state")
            .logical_position(),
        CanvasPoint::new(0.0, 600.0)
    );
}

#[cfg(feature = "udev")]
#[test]
fn pointer_clamp_falls_back_to_viewport_without_outputs() {
    let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
    let display: Display<EvilWm> = Display::new().expect("display");
    let state = EvilWm::new(&mut event_loop, display, None, None).expect("state");

    let clamped = state.clamp_pointer_to_output_layout_or_viewport((-50.0, 9999.0).into());
    assert_eq!(clamped.x, 0.0);
    assert_eq!(clamped.y, 720.0);
}

#[cfg(feature = "lua")]
#[test]
fn pointer_clamp_uses_full_output_layout_bounds() {
    let mut state = create_live_test_state(None);
    let left = create_test_output("left", (0, 0), (800, 600));
    let right = create_test_output("right", (800, 0), (1024, 768));
    state.space.map_output(&left, (0, 0));
    state.space.map_output(&right, (800, 0));
    state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
    state.register_output_state(
        "right",
        CanvasPoint::new(800.0, 0.0),
        Size::new(1024.0, 768.0),
    );

    let clamped = state.clamp_pointer_to_output_layout_or_viewport((2500.0, -50.0).into());
    assert_eq!(clamped.x, 1824.0);
    assert_eq!(clamped.y, 0.0);
}

#[cfg(feature = "lua")]
#[test]
fn output_at_world_position_uses_visible_world_for_each_output() {
    let mut state = create_live_test_state(None);
    let left = create_test_output("left", (0, 0), (800, 600));
    let right = create_test_output("right", (800, 0), (800, 600));
    state.space.map_output(&left, (0, 0));
    state.space.map_output(&right, (800, 0));
    state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
    state.register_output_state(
        "right",
        CanvasPoint::new(800.0, 0.0),
        Size::new(800.0, 600.0),
    );
    state
        .output_state_for_name_mut("right")
        .expect("right output state")
        .viewport_mut()
        .pan_world(crate::canvas::Vec2::new(800.0, 0.0));

    let left_output = state
        .output_at_world_position(crate::canvas::Point::new(120.0, 120.0))
        .expect("left output");
    let right_output = state
        .output_at_world_position(crate::canvas::Point::new(1020.0, 140.0))
        .expect("right output");

    assert_eq!(left_output.name(), "left");
    assert_eq!(right_output.name(), "right");
}

#[cfg(feature = "lua")]
#[test]
fn output_relayout_updates_window_output_association_rule() {
    let mut state = create_live_test_state(None);
    let left = create_test_output("left", (0, 0), (800, 600));
    let right = create_test_output("right", (800, 0), (800, 600));
    state.space.map_output(&left, (0, 0));
    state.space.map_output(&right, (800, 0));
    state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
    state.register_output_state(
        "right",
        CanvasPoint::new(800.0, 0.0),
        Size::new(800.0, 600.0),
    );
    state
        .output_state_for_name_mut("right")
        .expect("right output state")
        .viewport_mut()
        .pan_world(crate::canvas::Vec2::new(800.0, 0.0));

    let bounds = crate::canvas::Rect::new(900.0, 100.0, 200.0, 200.0);
    assert_eq!(
        state.output_id_for_window_bounds(bounds).as_deref(),
        Some("right")
    );

    state.sync_output_state(
        "right",
        CanvasPoint::new(1800.0, 0.0),
        Size::new(800.0, 600.0),
    );
    assert_eq!(
        state.output_id_for_window_bounds(bounds).as_deref(),
        Some("right")
    );

    state
        .output_state_for_name_mut("right")
        .expect("right output state")
        .viewport_mut()
        .pan_world(crate::canvas::Vec2::new(1200.0, 0.0));
    assert_eq!(state.output_id_for_window_bounds(bounds), None);
}

#[cfg(feature = "lua")]
#[test]
fn output_state_event_log_records_registration_and_update() {
    let log_dir = tempfile::tempdir().expect("tempdir");
    let log_path = log_dir.path().join("events.jsonl");
    let mut state = create_live_test_state(None);
    state.event_log_path = Some(log_path.clone());
    let left = create_test_output("left", (0, 0), (800, 600));
    state.space.map_output(&left, (0, 0));
    state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
    state.sync_output_state(
        "left",
        CanvasPoint::new(100.0, 50.0),
        Size::new(1024.0, 768.0),
    );

    let contents = std::fs::read_to_string(&log_path).expect("read event log");
    let entries = contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect::<Vec<_>>();
    assert!(
        entries
            .iter()
            .any(|entry| entry["kind"] == "output_registered")
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["kind"] == "output_updated")
    );
}

#[cfg(feature = "lua")]
#[test]
fn window_output_association_uses_center_point_inside_visible_world() {
    let mut state = create_live_test_state(None);
    let left = create_test_output("left", (0, 0), (800, 600));
    let right = create_test_output("right", (800, 0), (800, 600));
    state.space.map_output(&left, (0, 0));
    state.space.map_output(&right, (800, 0));
    state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
    state.register_output_state(
        "right",
        CanvasPoint::new(800.0, 0.0),
        Size::new(800.0, 600.0),
    );
    state
        .output_state_for_name_mut("right")
        .expect("right output state")
        .viewport_mut()
        .pan_world(crate::canvas::Vec2::new(800.0, 0.0));

    let left_id =
        state.output_id_for_window_bounds(crate::canvas::Rect::new(100.0, 100.0, 200.0, 200.0));
    let right_id =
        state.output_id_for_window_bounds(crate::canvas::Rect::new(1000.0, 100.0, 200.0, 200.0));

    assert_eq!(left_id.as_deref(), Some("left"));
    assert_eq!(right_id.as_deref(), Some("right"));
}

#[cfg(feature = "lua")]
#[test]
fn output_management_state_tracks_current_output_positions_and_modes() {
    let mut state = create_live_test_state(None);
    let left = create_test_output("left", (0, 0), (800, 600));
    let right = create_test_output("right", (800, 0), (1024, 768));
    state.space.map_output(&left, (0, 0));
    state.space.map_output(&right, (800, 0));
    state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
    state.register_output_state(
        "right",
        CanvasPoint::new(800.0, 0.0),
        Size::new(1024.0, 768.0),
    );
    state.notify_output_management_state();

    let heads = state
        .output_management_protocol_state
        .current_state_for_tests();
    let left_head = heads.get("left").expect("left head");
    let right_head = heads.get("right").expect("right head");

    assert_eq!(left_head.position, (0, 0));
    assert_eq!(right_head.position, (800, 0));
    assert_eq!(left_head.current_mode_index, Some(0));
    assert_eq!(right_head.current_mode_index, Some(0));
    assert_eq!(left_head.modes[0].width, 800);
    assert_eq!(left_head.modes[0].height, 600);
    assert_eq!(right_head.modes[0].width, 1024);
    assert_eq!(right_head.modes[0].height, 768);
}

#[cfg(feature = "lua")]
#[test]
fn output_management_state_updates_after_output_relayout() {
    let mut state = create_live_test_state(None);
    let right = create_test_output("right", (800, 0), (1024, 768));
    state.space.map_output(&right, (800, 0));
    state.register_output_state(
        "right",
        CanvasPoint::new(800.0, 0.0),
        Size::new(1024.0, 768.0),
    );
    state.notify_output_management_state();
    assert_eq!(
        state
            .output_management_protocol_state
            .current_state_for_tests()
            .get("right")
            .expect("right head")
            .position,
        (800, 0)
    );

    state.sync_output_state(
        "right",
        CanvasPoint::new(1800.0, 120.0),
        Size::new(1280.0, 720.0),
    );
    state.notify_output_management_state();
    let right_head = state
        .output_management_protocol_state
        .current_state_for_tests()
        .get("right")
        .expect("right head after relayout");
    assert_eq!(right_head.position, (1800, 120));
    assert_eq!(right_head.modes[0].width, 1280);
    assert_eq!(right_head.modes[0].height, 720);
}

#[cfg(feature = "udev")]
#[test]
fn sync_primary_output_state_uses_remaining_output_after_primary_removal() {
    let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
    let display: Display<EvilWm> = Display::new().expect("display");
    let mut state = EvilWm::new(&mut event_loop, display, None, None).expect("state");
    let left = create_test_output("left", (0, 0), (800, 600));
    let right = create_test_output("right", (800, 0), (1024, 768));
    state.space.map_output(&left, (0, 0));
    state.space.map_output(&right, (800, 0));
    state.sync_primary_output_state_from_space();
    assert!(
        state.output_states.contains_key("left"),
        "left output should be tracked"
    );

    state.space.unmap_output(&left);
    state.sync_primary_output_state_from_space();
    assert!(
        !state.output_states.contains_key("left"),
        "left output should be pruned after removal"
    );
    assert!(
        state.output_states.contains_key("right"),
        "right output should still be tracked"
    );
    let right_state = state.output_state_for_name("right").expect("right state");
    assert_eq!(right_state.logical_position(), CanvasPoint::new(800.0, 0.0));
}

#[test]
fn resize_edges_from_maps_corner_edges() {
    let edges = resize_edges_from(xdg_toplevel::ResizeEdge::TopLeft);
    assert!(edges.left);
    assert!(edges.top);
    assert!(!edges.right);
    assert!(!edges.bottom);

    let edges = resize_edges_from(xdg_toplevel::ResizeEdge::BottomRight);
    assert!(edges.right);
    assert!(edges.bottom);
    assert!(!edges.left);
    assert!(!edges.top);
}

#[test]
fn validate_ipc_screenshot_path_allows_tmp_and_rejects_outside_roots() {
    let tmp_path = std::env::temp_dir().join("evilwm-test-capture.ppm");
    let validated = super::validate_ipc_screenshot_path(&tmp_path).expect("tmp screenshot path");
    assert!(validated.starts_with(std::env::temp_dir()));

    let rejected = super::validate_ipc_screenshot_path(std::path::Path::new("/etc/evilwm.ppm"));
    assert!(rejected.is_err());
}

#[test]
fn ipc_socket_permissions_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
    let display: Display<EvilWm> = Display::new().expect("display");
    let state = EvilWm::new(&mut event_loop, display, None, None).expect("state");

    let mode = std::fs::metadata(&state.ipc_socket_path)
        .expect("ipc socket metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[cfg(feature = "lua")]
#[test]
fn live_hook_errors_are_captured_in_runtime_snapshot() {
    let mut state = create_live_test_state(None);
    let hooks = LiveLuaHooks::new(".").expect("live hooks");
    hooks
        .load_script_str(
            r#"
                evil.on.key = function(ctx)
                  error("boom")
                end
                "#,
            "hook-error.lua",
        )
        .expect("load hooks");
    state.live_lua = Some(hooks);

    assert!(!state.trigger_live_key("Super+H".to_string()));

    let snapshot = crate::ipc::RuntimeSnapshot::from_live(&state);
    assert_eq!(snapshot.hook_errors.len(), 1);
    assert_eq!(snapshot.hook_errors[0].hook, "key");
    assert_eq!(snapshot.hook_errors[0].count, 1);
    assert!(snapshot.hook_errors[0].last_error.contains("evil.on.key"));
}

#[cfg(feature = "lua")]
#[test]
fn live_hook_error_messages_include_hook_name_and_error() {
    let error = ConfigError::Validation("boom".into());
    assert_eq!(
        format_live_hook_error("key", &error),
        "[evilwm] lua hook error: evil.on.key — config validation error: boom"
    );
}

#[cfg(feature = "lua")]
#[test]
fn headless_and_live_hook_payloads_match_for_shared_core_fields() {
    let config = Config {
        backend: Some("winit".into()),
        canvas: CanvasConfig::default(),
        draw: DrawConfig::default(),
        window: crate::lua::WindowConfig::default(),
        placement: crate::lua::PlacementConfig::default(),
        tty: crate::lua::TtyConfig::default(),
        autostart: Vec::new(),
        bindings: vec![BindingConfig {
            mods: vec!["Super".into()],
            key: "H".into(),
            action: "pan_left".into(),
            amount: None,
            command: None,
        }],
        rules: Vec::new(),
        source_root: std::path::PathBuf::from("."),
    };

    let mut headless = run_headless(HeadlessOptions {
        config: Some(config.clone()),
        ..HeadlessOptions::default()
    });
    let first = headless.create_window(
        crate::canvas::Rect::new(20.0, 30.0, 240.0, 160.0),
        crate::window::WindowProperties {
            title: Some("first".into()),
            ..crate::window::WindowProperties::default()
        },
    );
    let second = headless.create_window(
        crate::canvas::Rect::new(320.0, 40.0, 260.0, 180.0),
        crate::window::WindowProperties {
            title: Some("second".into()),
            ..crate::window::WindowProperties::default()
        },
    );
    headless.focus_window(second);
    let headless_session = std::rc::Rc::new(std::cell::RefCell::new(headless));
    let lua = LuaSession::new(".", headless_session.clone()).expect("lua session");

    let mut live = create_live_test_state(Some(config));
    live.window_models.insert(
        first,
        Window::new(first, crate::canvas::Rect::new(20.0, 30.0, 240.0, 160.0)),
    );
    live.window_models.insert(
        second,
        Window::new(second, crate::canvas::Rect::new(320.0, 40.0, 260.0, 180.0)),
    );
    live.window_models
        .get_mut(&first)
        .expect("first live window")
        .properties
        .title = Some("first".into());
    live.window_models
        .get_mut(&second)
        .expect("second live window")
        .properties
        .title = Some("second".into());
    live.focus_stack.focus(second);

    let script = r#"
        captured = {}

        evil.on.focus_changed = function(ctx)
          captured.focus = {
            previous = ctx.previous_window_id,
            current = ctx.focused_window_id,
            current_title = ctx.focused_window and ctx.focused_window.title or nil,
          }
        end

        evil.on.move_update = function(ctx)
          captured.move = {
            dx = ctx.dx,
            dy = ctx.dy,
            pointer_x = ctx.pointer and ctx.pointer.x or nil,
            pointer_y = ctx.pointer and ctx.pointer.y or nil,
            id = ctx.window.id,
          }
        end

        evil.on.resize_update = function(ctx)
          captured.resize = {
            left = ctx.edges.left,
            right = ctx.edges.right,
            top = ctx.edges.top,
            bottom = ctx.edges.bottom,
            pointer_x = ctx.pointer and ctx.pointer.x or nil,
          }
        end

        evil.on.key = function(ctx)
          captured.key = {
            keyspec = ctx.keyspec,
            action = ctx.bound_action,
            action_alias = ctx.action,
            has_binding = ctx.has_binding,
            super = ctx.modifiers.super,
            modifier_count = ctx.modifiers.count,
            pointer_x = ctx.pointer.x,
            pointer_has_output = ctx.pointer.output_id ~= nil,
          }
        end

        evil.on.window_property_changed = function(ctx)
          captured.property = {
            property = ctx.property,
            old_value = ctx.old_value,
            new_value = ctx.new_value,
            title = ctx.window.title,
          }
        end
        "#;
    lua.eval(script, "parity.lua")
        .expect("load headless parity hooks");

    let hooks = LiveLuaHooks::new(".").expect("live hooks");
    hooks
        .load_script_str(script, "parity.lua")
        .expect("load live parity hooks");
    live.live_lua = Some(hooks);

    let delta = crate::canvas::Vec2::new(18.0, -6.0);
    let pointer = CanvasPoint::new(360.0, 210.0);
    let edges = crate::window::ResizeEdges {
        left: false,
        right: true,
        top: false,
        bottom: true,
    };

    assert!(
        lua.trigger_focus_changed(Some(first), Some(second))
            .expect("headless focus")
    );
    assert!(
        lua.trigger_move_update(second, delta, Some(pointer))
            .expect("headless move")
    );
    assert!(
        lua.trigger_resize_update(second, delta, Some(pointer), edges)
            .expect("headless resize")
    );
    assert!(lua.trigger_key("Super+H").expect("headless key"));
    headless_session
        .borrow_mut()
        .window_models
        .get_mut(&second)
        .expect("headless second window")
        .properties
        .title = Some("second-renamed".into());
    assert!(
        lua.trigger_window_property_changed(
            second,
            "title",
            &PropertyValue::OptionString(Some("second".into())),
            &PropertyValue::OptionString(Some("second-renamed".into())),
        )
        .expect("headless property")
    );

    live.trigger_live_focus_changed(Some(first), Some(second));
    live.trigger_live_move_update(second, delta, pointer);
    live.trigger_live_resize_update(second, delta, pointer, edges);
    assert!(live.trigger_live_key("Super+H".to_string()));
    live.window_models
        .get_mut(&second)
        .expect("live second window")
        .properties
        .title = Some("second-renamed".into());
    live.trigger_live_window_property_changed(
        second,
        "title",
        PropertyValue::OptionString(Some("second".into())),
        PropertyValue::OptionString(Some("second-renamed".into())),
    );

    let headless_value = lua
        .eval("return captured", "read-headless.lua")
        .expect("read headless captured");
    let mlua::Value::Table(headless_table) = headless_value else {
        panic!("expected headless captured table");
    };

    let live_hooks = live.live_lua.as_ref().expect("live hooks still installed");
    let live_table: mlua::Table = live_hooks
        .lua_for_tests()
        .globals()
        .get("captured")
        .expect("read live captured");

    let compare_focus = |table: &mlua::Table| {
        let focus: mlua::Table = table.get("focus").expect("focus");
        (
            focus.get::<u64>("previous").expect("previous"),
            focus.get::<u64>("current").expect("current"),
            focus.get::<String>("current_title").expect("current_title"),
        )
    };
    let compare_move = |table: &mlua::Table| {
        let move_ctx: mlua::Table = table.get("move").expect("move");
        (
            move_ctx.get::<f64>("dx").expect("dx"),
            move_ctx.get::<f64>("dy").expect("dy"),
            move_ctx.get::<f64>("pointer_x").expect("pointer_x"),
            move_ctx.get::<f64>("pointer_y").expect("pointer_y"),
            move_ctx.get::<u64>("id").expect("id"),
        )
    };
    let compare_resize = |table: &mlua::Table| {
        let resize: mlua::Table = table.get("resize").expect("resize");
        (
            resize.get::<bool>("left").expect("left"),
            resize.get::<bool>("right").expect("right"),
            resize.get::<bool>("top").expect("top"),
            resize.get::<bool>("bottom").expect("bottom"),
            resize.get::<f64>("pointer_x").expect("pointer_x"),
        )
    };
    let compare_key = |table: &mlua::Table| {
        let key: mlua::Table = table.get("key").expect("key");
        (
            key.get::<String>("keyspec").expect("keyspec"),
            key.get::<String>("action").expect("action"),
            key.get::<String>("action_alias").expect("action_alias"),
            key.get::<bool>("has_binding").expect("has_binding"),
            key.get::<bool>("super").expect("super"),
            key.get::<i64>("modifier_count").expect("modifier_count"),
            key.get::<f64>("pointer_x").expect("pointer_x"),
            key.get::<bool>("pointer_has_output")
                .expect("pointer_has_output"),
        )
    };
    let compare_property = |table: &mlua::Table| {
        let property: mlua::Table = table.get("property").expect("property");
        (
            property.get::<String>("property").expect("property name"),
            property.get::<String>("old_value").expect("old_value"),
            property.get::<String>("new_value").expect("new_value"),
            property.get::<String>("title").expect("title"),
        )
    };

    assert_eq!(compare_focus(&headless_table), compare_focus(&live_table));
    assert_eq!(compare_move(&headless_table), compare_move(&live_table));
    assert_eq!(compare_resize(&headless_table), compare_resize(&live_table));
    assert_eq!(compare_key(&headless_table), compare_key(&live_table));
    assert_eq!(
        compare_property(&headless_table),
        compare_property(&live_table)
    );
}

#[cfg(feature = "lua")]
#[test]
fn move_hook_context_contains_correct_delta_and_pointer() {
    let mut state = create_live_test_state(None);
    let id = WindowId(20);
    state.window_models.insert(
        id,
        Window::new(id, crate::canvas::Rect::new(100.0, 200.0, 300.0, 200.0)),
    );

    let hooks = LiveLuaHooks::new(".").expect("live hooks");
    hooks
        .load_script_str(
            r#"
                local results = {}
                evil.on.move_begin = function(ctx)
                  results.begin_delta_x = ctx.delta.x
                  results.begin_delta_y = ctx.delta.y
                  results.begin_id = ctx.window.id
                end
                evil.on.move_update = function(ctx)
                  results.update_delta_x = ctx.delta.x
                  results.update_delta_y = ctx.delta.y
                  results.update_pointer_x = ctx.pointer.x
                  results.update_pointer_y = ctx.pointer.y
                end
                evil.on.move_end = function(ctx)
                  results.end_delta_x = ctx.delta.x
                  results.end_delta_y = ctx.delta.y
                end

                -- expose results for assertion
                _G._test_results = results
                "#,
            "move-context.lua",
        )
        .expect("load move context hooks");
    state.live_lua = Some(hooks);

    state.trigger_live_move_begin(id);
    state.trigger_live_move_update(
        id,
        crate::canvas::Vec2::new(15.0, -8.0),
        CanvasPoint::new(115.0, 192.0),
    );
    state.trigger_live_move_end(
        id,
        crate::canvas::Vec2::new(30.0, -16.0),
        CanvasPoint::new(130.0, 184.0),
    );

    // Read the captured values from Lua
    let hooks = state.live_lua.as_ref().unwrap();
    let results: mlua::Table = hooks
        .lua_for_tests()
        .globals()
        .get::<mlua::Table>("_test_results")
        .expect("test results table");

    assert_eq!(results.get::<f64>("begin_delta_x").unwrap(), 0.0);
    assert_eq!(results.get::<f64>("begin_delta_y").unwrap(), 0.0);
    assert_eq!(results.get::<u64>("begin_id").unwrap(), 20);
    assert_eq!(results.get::<f64>("update_delta_x").unwrap(), 15.0);
    assert_eq!(results.get::<f64>("update_delta_y").unwrap(), -8.0);
    assert_eq!(results.get::<f64>("update_pointer_x").unwrap(), 115.0);
    assert_eq!(results.get::<f64>("update_pointer_y").unwrap(), 192.0);
    assert_eq!(results.get::<f64>("end_delta_x").unwrap(), 30.0);
    assert_eq!(results.get::<f64>("end_delta_y").unwrap(), -16.0);
}

#[cfg(feature = "lua")]
#[test]
fn resize_hook_context_contains_correct_edges() {
    let mut state = create_live_test_state(None);
    let id = WindowId(21);
    state.window_models.insert(
        id,
        Window::new(id, crate::canvas::Rect::new(0.0, 0.0, 400.0, 300.0)),
    );

    let hooks = LiveLuaHooks::new(".").expect("live hooks");
    hooks
        .load_script_str(
            r#"
                local results = {}
                evil.on.resize_begin = function(ctx)
                  results.begin_right = ctx.edges.right
                  results.begin_bottom = ctx.edges.bottom
                  results.begin_left = ctx.edges.left
                  results.begin_top = ctx.edges.top
                end
                evil.on.resize_update = function(ctx)
                  results.update_right = ctx.edges.right
                  results.update_bottom = ctx.edges.bottom
                end
                evil.on.resize_end = function(ctx)
                  results.end_right = ctx.edges.right
                  results.end_bottom = ctx.edges.bottom
                end
                _G._test_edges = results
                "#,
            "resize-edges.lua",
        )
        .expect("load resize edge hooks");
    state.live_lua = Some(hooks);

    let edges = crate::window::ResizeEdges {
        left: false,
        right: true,
        top: false,
        bottom: true,
    };
    state.trigger_live_resize_begin(id, edges);
    state.trigger_live_resize_update(
        id,
        crate::canvas::Vec2::new(10.0, 5.0),
        CanvasPoint::new(10.0, 5.0),
        edges,
    );
    state.trigger_live_resize_end(
        id,
        crate::canvas::Vec2::new(20.0, 10.0),
        CanvasPoint::new(20.0, 10.0),
        edges,
    );

    let hooks = state.live_lua.as_ref().unwrap();
    let results: mlua::Table = hooks
        .lua_for_tests()
        .globals()
        .get::<mlua::Table>("_test_edges")
        .expect("test edges table");

    assert!(results.get::<bool>("begin_right").unwrap());
    assert!(results.get::<bool>("begin_bottom").unwrap());
    assert!(!results.get::<bool>("begin_left").unwrap());
    assert!(!results.get::<bool>("begin_top").unwrap());
    assert!(results.get::<bool>("update_right").unwrap());
    assert!(results.get::<bool>("update_bottom").unwrap());
    assert!(results.get::<bool>("end_right").unwrap());
    assert!(results.get::<bool>("end_bottom").unwrap());
}

#[cfg(feature = "lua")]
#[test]
fn move_without_hooks_does_not_move_window() {
    let mut state = create_live_test_state(None);
    let id = WindowId(22);
    state.window_models.insert(
        id,
        Window::new(id, crate::canvas::Rect::new(50.0, 60.0, 300.0, 200.0)),
    );
    // No hooks loaded — move should be inert.
    state.trigger_live_move_begin(id);
    state.trigger_live_move_update(
        id,
        crate::canvas::Vec2::new(100.0, 50.0),
        CanvasPoint::new(150.0, 110.0),
    );
    state.trigger_live_move_end(
        id,
        crate::canvas::Vec2::new(100.0, 50.0),
        CanvasPoint::new(150.0, 110.0),
    );

    // Window should not have moved.
    let window = state.window_models.get(&id).expect("window exists");
    assert_eq!(window.bounds.origin, CanvasPoint::new(50.0, 60.0));
}

#[cfg(feature = "lua")]
#[test]
fn windows_survive_output_removal_without_state_corruption() {
    let mut state = create_live_test_state(None);
    let left = create_test_output("left", (0, 0), (800, 600));
    let right = create_test_output("right", (800, 0), (1024, 768));
    state.space.map_output(&left, (0, 0));
    state.space.map_output(&right, (800, 0));
    state.register_output_state(
        "left",
        CanvasPoint::new(0.0, 0.0),
        crate::canvas::Size::new(800.0, 600.0),
    );
    state.register_output_state(
        "right",
        CanvasPoint::new(800.0, 0.0),
        crate::canvas::Size::new(1024.0, 768.0),
    );

    let id = WindowId(50);
    state.window_models.insert(
        id,
        Window::new(id, crate::canvas::Rect::new(100.0, 100.0, 300.0, 200.0)),
    );
    state.focus_stack.focus(id);

    // Window should be associated with 'left' output.
    let snapshot_before = state.state_snapshot();
    let win_before = snapshot_before
        .windows
        .iter()
        .find(|w| w.id == 50)
        .expect("window");
    assert_eq!(win_before.output_id.as_deref(), Some("left"));

    // Remove the 'left' output.
    state.space.unmap_output(&left);
    state.output_states.remove("left");

    // Window should still exist, focus should be intact.
    assert!(state.window_models.contains_key(&id));
    assert_eq!(state.focus_stack.focused(), Some(id));

    // Snapshot should still contain the window, but output_id may be None
    // since the associated output is gone.
    let snapshot_after = state.state_snapshot();
    assert_eq!(snapshot_after.windows.len(), 1);
    let _win_after = snapshot_after
        .windows
        .iter()
        .find(|w| w.id == 50)
        .expect("window");
    // The window's center (250, 200) is no longer inside any output's visible world
    // since 'left' was removed, so output_id should reflect reality.
    assert_eq!(snapshot_after.outputs.len(), 1);
    assert_eq!(snapshot_after.outputs[0].id, "right");
}

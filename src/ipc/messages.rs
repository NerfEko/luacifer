use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    canvas::{Rect, Size},
    headless::HeadlessSession,
};
#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
use crate::{compositor::EvilWm, window::Window};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookErrorSnapshot {
    pub hook: String,
    pub count: u64,
    pub last_error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub backend: String,
    pub config_loaded: bool,
    pub config_path: Option<PathBuf>,
    pub screen_size: Size,
    pub window_count: usize,
    pub focused_window: Option<u64>,
    pub zoom: f64,
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub bindings: usize,
    pub rules: usize,
    pub autostart: usize,
    pub visible_world: Rect,
    pub next_placement: Rect,
    pub session_locked: bool,
    pub idle_inhibited: bool,
    pub hook_errors: Vec<HookErrorSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcRequest {
    GetRuntimeSnapshot,
    Quit,
    Lock,
    Unlock,
    Screenshot { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    RuntimeSnapshot { snapshot: Box<RuntimeSnapshot> },
    Ok { message: String },
    Error { message: String },
}

impl RuntimeSnapshot {
    pub fn from_headless(session: &HeadlessSession) -> Self {
        Self {
            backend: "headless".into(),
            config_loaded: session.config.is_some(),
            config_path: session.config_path.clone(),
            screen_size: session.output_state.viewport().screen_size(),
            window_count: session.window_models.len(),
            focused_window: session.focus_stack.focused().map(|id| id.0),
            zoom: session.viewport().zoom(),
            #[cfg(feature = "lua")]
            min_zoom: session
                .config
                .as_ref()
                .map_or(0.1, |cfg| cfg.canvas.min_zoom),
            #[cfg(not(feature = "lua"))]
            min_zoom: 0.1,
            #[cfg(feature = "lua")]
            max_zoom: session
                .config
                .as_ref()
                .map_or(8.0, |cfg| cfg.canvas.max_zoom),
            #[cfg(not(feature = "lua"))]
            max_zoom: 8.0,
            #[cfg(feature = "lua")]
            bindings: session.config.as_ref().map_or(0, |cfg| cfg.bindings.len()),
            #[cfg(not(feature = "lua"))]
            bindings: 0,
            #[cfg(feature = "lua")]
            rules: session.config.as_ref().map_or(0, |cfg| cfg.rules.len()),
            #[cfg(not(feature = "lua"))]
            rules: 0,
            #[cfg(feature = "lua")]
            autostart: session.config.as_ref().map_or(0, |cfg| cfg.autostart.len()),
            #[cfg(not(feature = "lua"))]
            autostart: 0,
            visible_world: session.viewport().visible_world_rect(),
            next_placement: session.next_placement(),
            session_locked: false,
            idle_inhibited: false,
            hook_errors: Vec::new(),
        }
    }

    #[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
    pub fn from_live(state: &EvilWm) -> Self {
        let existing = state
            .window_models
            .values()
            .cloned()
            .collect::<Vec<Window>>();
        let next_placement = state
            .fallback_placement_policy
            .place_new_window(state.viewport(), &existing, None)
            .bounds;

        Self {
            backend: if cfg!(feature = "udev") && state.is_tty_backend() {
                "udev".into()
            } else {
                "winit".into()
            },
            config_loaded: state.config.is_some(),
            config_path: state.config_path.clone(),
            screen_size: state.output_state.viewport().screen_size(),
            window_count: state.window_models.len(),
            focused_window: state.focus_stack.focused().map(|id| id.0),
            zoom: state.viewport().zoom(),
            #[cfg(feature = "lua")]
            min_zoom: state.config.as_ref().map_or(0.1, |cfg| cfg.canvas.min_zoom),
            #[cfg(not(feature = "lua"))]
            min_zoom: 0.1,
            #[cfg(feature = "lua")]
            max_zoom: state.config.as_ref().map_or(8.0, |cfg| cfg.canvas.max_zoom),
            #[cfg(not(feature = "lua"))]
            max_zoom: 8.0,
            #[cfg(feature = "lua")]
            bindings: state.config.as_ref().map_or(0, |cfg| cfg.bindings.len()),
            #[cfg(not(feature = "lua"))]
            bindings: 0,
            #[cfg(feature = "lua")]
            rules: state.config.as_ref().map_or(0, |cfg| cfg.rules.len()),
            #[cfg(not(feature = "lua"))]
            rules: 0,
            #[cfg(feature = "lua")]
            autostart: state.config.as_ref().map_or(0, |cfg| cfg.autostart.len()),
            #[cfg(not(feature = "lua"))]
            autostart: 0,
            visible_world: state.viewport().visible_world_rect(),
            next_placement,
            session_locked: state.session_locked(),
            idle_inhibited: state.idle_inhibited(),
            #[cfg(feature = "lua")]
            hook_errors: state.live_hook_errors_snapshot(),
            #[cfg(not(feature = "lua"))]
            hook_errors: Vec::new(),
        }
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

impl IpcRequest {
    pub fn from_json(input: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(input)
    }
}

impl IpcResponse {
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{HookErrorSnapshot, IpcRequest, IpcResponse, RuntimeSnapshot};
    use crate::canvas::{Rect, Size};
    use std::path::PathBuf;

    #[test]
    fn ipc_request_roundtrips_json() {
        for request in [
            IpcRequest::GetRuntimeSnapshot,
            IpcRequest::Quit,
            IpcRequest::Lock,
            IpcRequest::Unlock,
            IpcRequest::Screenshot {
                path: PathBuf::from("/tmp/test.ppm"),
            },
        ] {
            let json = serde_json::to_string(&request).expect("serialize request");
            let reparsed = IpcRequest::from_json(&json).expect("parse request");
            assert_eq!(reparsed, request);
        }
    }

    #[test]
    fn ipc_request_rejects_unknown_type() {
        let error = IpcRequest::from_json(r#"{"type":"does_not_exist"}"#)
            .expect_err("unknown request must fail");
        assert!(error.to_string().contains("unknown variant"));
    }

    #[test]
    fn ipc_response_serializes_snapshot_payload() {
        let response = IpcResponse::RuntimeSnapshot {
            snapshot: Box::new(RuntimeSnapshot {
                backend: "winit".into(),
                config_loaded: true,
                config_path: None,
                screen_size: Size::new(1280.0, 720.0),
                window_count: 2,
                focused_window: Some(1),
                zoom: 1.0,
                min_zoom: 0.1,
                max_zoom: 8.0,
                bindings: 3,
                rules: 1,
                autostart: 0,
                visible_world: Rect::new(0.0, 0.0, 1280.0, 720.0),
                next_placement: Rect::new(10.0, 20.0, 300.0, 200.0),
                session_locked: false,
                idle_inhibited: false,
                hook_errors: vec![HookErrorSnapshot {
                    hook: "key".into(),
                    count: 2,
                    last_error: "boom".into(),
                }],
            }),
        };

        let json = response.to_json_pretty().expect("serialize response");
        assert!(json.contains("runtime_snapshot"));
        assert!(json.contains("\"backend\": \"winit\""));
    }
}

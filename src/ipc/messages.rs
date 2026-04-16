use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    canvas::{Rect, Size},
    headless::HeadlessSession,
};

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
    pub preview_window_bounds: Rect,
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
            preview_window_bounds: session.preview_window_bounds(),
        }
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

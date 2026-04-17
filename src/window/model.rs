use std::time::SystemTime;

use crate::canvas::{Rect, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

/// Properties sourced from the client or backend.
///
/// - `app_id`, `title`: guaranteed on XDG toplevels.
/// - `pid`: optional/backend-dependent; `None` in headless and when unavailable in the live
///   compositor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowProperties {
    pub app_id: Option<String>,
    pub title: Option<String>,
    /// Process ID of the client that owns this window.
    /// Optional/backend-dependent — populated when the runtime can determine it cleanly.
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub id: WindowId,
    pub properties: WindowProperties,
    pub bounds: Rect,
    pub floating: bool,
    pub exclude_from_focus: bool,
    /// Whether this window is in fullscreen state. Initially false.
    /// In the live compositor this reflects the XDG fullscreen request.
    pub fullscreen: bool,
    /// Whether this window is in maximized state. Initially false.
    pub maximized: bool,
    /// Whether this window has an urgency hint set. Initially false.
    pub urgent: bool,
    /// Wall-clock time at which this window was first mapped into the session.
    /// Set automatically in `Window::new`; always `Some` for windows in the active session.
    pub mapped_at: Option<SystemTime>,
    /// Wall-clock time at which this window most recently gained focus.
    /// `None` until the first focus event.
    pub last_focused_at: Option<SystemTime>,
}

impl Window {
    pub fn new(id: WindowId, bounds: Rect) -> Self {
        Self {
            id,
            properties: WindowProperties::default(),
            bounds,
            floating: true,
            exclude_from_focus: false,
            fullscreen: false,
            maximized: false,
            urgent: false,
            mapped_at: Some(SystemTime::now()),
            last_focused_at: None,
        }
    }

    pub fn with_properties(mut self, properties: WindowProperties) -> Self {
        self.properties = properties;
        self
    }

    pub fn size(&self) -> Size {
        self.bounds.size
    }
}

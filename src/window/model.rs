use crate::canvas::{Rect, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowProperties {
    pub app_id: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub id: WindowId,
    pub properties: WindowProperties,
    pub bounds: Rect,
    pub floating: bool,
    pub exclude_from_focus: bool,
}

impl Window {
    pub fn new(id: WindowId, bounds: Rect) -> Self {
        Self {
            id,
            properties: WindowProperties::default(),
            bounds,
            floating: true,
            exclude_from_focus: false,
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

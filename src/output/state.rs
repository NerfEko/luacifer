use crate::canvas::{Point, Size, Viewport};

#[derive(Debug, Clone, PartialEq)]
pub struct OutputState {
    name: String,
    logical_position: Point,
    viewport: Viewport,
}

impl OutputState {
    pub fn new(name: impl Into<String>, logical_position: Point, screen_size: Size) -> Self {
        Self {
            name: name.into(),
            logical_position,
            viewport: Viewport::new(screen_size),
        }
    }

    pub fn with_viewport(
        name: impl Into<String>,
        logical_position: Point,
        viewport: Viewport,
    ) -> Self {
        Self {
            name: name.into(),
            logical_position,
            viewport,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn logical_position(&self) -> Point {
        self.logical_position
    }

    pub fn set_logical_position(&mut self, logical_position: Point) {
        self.logical_position = logical_position;
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    pub fn viewport(&self) -> &Viewport {
        &self.viewport
    }

    pub fn viewport_mut(&mut self) -> &mut Viewport {
        &mut self.viewport
    }

    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }
}

use crate::{
    canvas::{Point, Rect, Size, Vec2, Viewport},
    window::model::Window,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacementTarget {
    pub bounds: Rect,
}

/// Fallback/reference placement used when Lua policy has not supplied a
/// placement decision yet.
///
/// In the Lua-first rewrite this is intentionally a minimal safety net, not
/// the long-term authority for window placement strategy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacementPolicy {
    pub default_size: Size,
    pub padding: f64,
    pub cascade_step: Vec2,
}

impl Default for PlacementPolicy {
    fn default() -> Self {
        Self {
            default_size: Size::new(900.0, 600.0),
            padding: 32.0,
            cascade_step: Vec2::new(32.0, 24.0),
        }
    }
}

impl PlacementPolicy {
    /// Produce a fallback placement for a newly mapped window.
    ///
    /// Lua hooks may immediately replace this location after map.
    pub fn place_new_window(
        &self,
        viewport: &Viewport,
        existing: &[Window],
        requested_size: Option<Size>,
    ) -> PlacementTarget {
        let visible = viewport.visible_world_rect();
        let max_w = (visible.size.w - self.padding * 2.0).max(1.0);
        let max_h = (visible.size.h - self.padding * 2.0).max(1.0);
        let requested = requested_size.unwrap_or(self.default_size);
        let size = Size::new(requested.w.min(max_w), requested.h.min(max_h));

        let mut origin = Point::new(
            visible.origin.x + (visible.size.w - size.w) / 2.0,
            visible.origin.y + (visible.size.h - size.h) / 2.0,
        );

        let cascade_factor = (existing.len() % 8) as f64;
        origin.x += self.cascade_step.x * cascade_factor;
        origin.y += self.cascade_step.y * cascade_factor;

        let max_x = visible.origin.x + visible.size.w - size.w - self.padding;
        let max_y = visible.origin.y + visible.size.h - size.h - self.padding;
        origin.x = origin.x.clamp(
            visible.origin.x + self.padding,
            max_x.max(visible.origin.x + self.padding),
        );
        origin.y = origin.y.clamp(
            visible.origin.y + self.padding,
            max_y.max(visible.origin.y + self.padding),
        );

        PlacementTarget {
            bounds: Rect::new(origin.x, origin.y, size.w, size.h),
        }
    }
}

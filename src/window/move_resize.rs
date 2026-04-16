use crate::canvas::{Rect, Size, Vec2};
use crate::window::model::Window;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizePolicy {
    pub min_size: Size,
    pub max_size: Option<Size>,
    pub snap_distance: f64,
}

impl Default for ResizePolicy {
    fn default() -> Self {
        Self {
            min_size: Size::new(120.0, 80.0),
            max_size: None,
            snap_distance: 16.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizeEdges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl ResizeEdges {
    pub const fn all() -> Self {
        Self {
            left: true,
            right: true,
            top: true,
            bottom: true,
        }
    }
}

impl Window {
    pub fn moved_by(&self, delta: Vec2) -> Self {
        let mut window = self.clone();
        window.bounds.origin.x += delta.x;
        window.bounds.origin.y += delta.y;
        window
    }

    pub fn resized_by(&self, delta: Vec2, edges: ResizeEdges, policy: ResizePolicy) -> Self {
        let mut bounds = self.bounds;

        if edges.left {
            bounds.origin.x += delta.x;
            bounds.size.w -= delta.x;
        }
        if edges.right {
            bounds.size.w += delta.x;
        }
        if edges.top {
            bounds.origin.y += delta.y;
            bounds.size.h -= delta.y;
        }
        if edges.bottom {
            bounds.size.h += delta.y;
        }

        if bounds.size.w < policy.min_size.w {
            let correction = policy.min_size.w - bounds.size.w;
            if edges.left {
                bounds.origin.x -= correction;
            }
            bounds.size.w = policy.min_size.w;
        }
        if bounds.size.h < policy.min_size.h {
            let correction = policy.min_size.h - bounds.size.h;
            if edges.top {
                bounds.origin.y -= correction;
            }
            bounds.size.h = policy.min_size.h;
        }

        if let Some(max_size) = policy.max_size {
            if bounds.size.w > max_size.w {
                let correction = bounds.size.w - max_size.w;
                if edges.left {
                    bounds.origin.x += correction;
                }
                bounds.size.w = max_size.w;
            }
            if bounds.size.h > max_size.h {
                let correction = bounds.size.h - max_size.h;
                if edges.top {
                    bounds.origin.y += correction;
                }
                bounds.size.h = max_size.h;
            }
        }

        let mut window = self.clone();
        window.bounds = bounds;
        window
    }
}

pub fn snap_to_rect(rect: Rect, target: Rect, snap_distance: f64) -> Rect {
    let mut snapped = rect;

    if (rect.origin.x - target.origin.x).abs() <= snap_distance {
        snapped.origin.x = target.origin.x;
    }
    if (rect.origin.y - target.origin.y).abs() <= snap_distance {
        snapped.origin.y = target.origin.y;
    }

    let rect_right = rect.origin.x + rect.size.w;
    let target_right = target.origin.x + target.size.w;
    if (rect_right - target_right).abs() <= snap_distance {
        snapped.origin.x = target_right - rect.size.w;
    }

    let rect_bottom = rect.origin.y + rect.size.h;
    let target_bottom = target.origin.y + target.size.h;
    if (rect_bottom - target_bottom).abs() <= snap_distance {
        snapped.origin.y = target_bottom - rect.size.h;
    }

    snapped
}

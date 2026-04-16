use crate::canvas::{Point, Vec2, Viewport};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    PanLeft { amount: f64 },
    PanRight { amount: f64 },
    PanUp { amount: f64 },
    PanDown { amount: f64 },
    ZoomIn { factor: f64 },
    ZoomOut { factor: f64 },
    CloseWindow,
    Spawn { command: String },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Self::PanLeft { .. } => "pan_left",
            Self::PanRight { .. } => "pan_right",
            Self::PanUp { .. } => "pan_up",
            Self::PanDown { .. } => "pan_down",
            Self::ZoomIn { .. } => "zoom_in",
            Self::ZoomOut { .. } => "zoom_out",
            Self::CloseWindow => "close_window",
            Self::Spawn { .. } => "spawn",
        }
    }

    pub fn from_name(
        name: &str,
        amount: Option<f64>,
        default_pan: f64,
        default_zoom: f64,
        command: Option<&str>,
    ) -> Option<Self> {
        match name {
            "pan_left" => Some(Self::PanLeft {
                amount: amount.unwrap_or(default_pan),
            }),
            "pan_right" => Some(Self::PanRight {
                amount: amount.unwrap_or(default_pan),
            }),
            "pan_up" => Some(Self::PanUp {
                amount: amount.unwrap_or(default_pan),
            }),
            "pan_down" => Some(Self::PanDown {
                amount: amount.unwrap_or(default_pan),
            }),
            "zoom_in" => Some(Self::ZoomIn {
                factor: amount.unwrap_or(default_zoom),
            }),
            "zoom_out" => Some(Self::ZoomOut {
                factor: amount.unwrap_or(1.0 / default_zoom),
            }),
            "close_window" => Some(Self::CloseWindow),
            "spawn" => Some(Self::Spawn {
                command: command?.to_string(),
            }),
            _ => None,
        }
    }

    pub fn apply_to_viewport(self, viewport: &mut Viewport) {
        match self {
            Self::PanLeft { amount } => viewport.pan_world(Vec2::new(-amount, 0.0)),
            Self::PanRight { amount } => viewport.pan_world(Vec2::new(amount, 0.0)),
            Self::PanUp { amount } => viewport.pan_world(Vec2::new(0.0, -amount)),
            Self::PanDown { amount } => viewport.pan_world(Vec2::new(0.0, amount)),
            Self::ZoomIn { factor } => viewport.zoom_at_screen(screen_center(viewport), factor),
            Self::ZoomOut { factor } => viewport.zoom_at_screen(screen_center(viewport), factor),
            Self::CloseWindow | Self::Spawn { .. } => {}
        }
    }
}

fn screen_center(viewport: &Viewport) -> Point {
    let size = viewport.screen_size();
    Point::new(size.w / 2.0, size.h / 2.0)
}

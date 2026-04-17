use crate::canvas::geometry::{Point, Rect, Size, Vec2};

#[derive(Debug, Clone, PartialEq)]
pub struct Viewport {
    world_origin: Point,
    screen_size: Size,
    zoom: f64,
    min_zoom: f64,
    max_zoom: f64,
}

impl Viewport {
    pub fn new(screen_size: Size) -> Self {
        Self {
            world_origin: Point::default(),
            screen_size,
            zoom: 1.0,
            min_zoom: 0.1,
            max_zoom: 8.0,
        }
    }

    pub fn try_with_zoom_limits(mut self, min_zoom: f64, max_zoom: f64) -> Result<Self, String> {
        if !min_zoom.is_finite() || min_zoom <= 0.0 {
            return Err("min_zoom must be a positive finite number".into());
        }
        if !max_zoom.is_finite() || max_zoom < min_zoom {
            return Err("max_zoom must be a finite number >= min_zoom".into());
        }
        self.min_zoom = min_zoom;
        self.max_zoom = max_zoom;
        self.zoom = self.zoom.clamp(self.min_zoom, self.max_zoom);
        Ok(self)
    }

    pub fn with_zoom_limits(self, min_zoom: f64, max_zoom: f64) -> Self {
        self.clone()
            .try_with_zoom_limits(min_zoom, max_zoom)
            .unwrap_or(self)
    }

    pub fn world_origin(&self) -> Point {
        self.world_origin
    }

    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    pub fn screen_size(&self) -> Size {
        self.screen_size
    }

    pub fn set_screen_size(&mut self, screen_size: Size) {
        self.screen_size = screen_size;
    }

    pub fn screen_to_world(&self, screen: Point) -> Point {
        self.world_origin + Vec2::new(screen.x / self.zoom, screen.y / self.zoom)
    }

    pub fn world_to_screen(&self, world: Point) -> Point {
        let delta = world - self.world_origin;
        Point::new(delta.x * self.zoom, delta.y * self.zoom)
    }

    pub fn visible_world_rect(&self) -> Rect {
        Rect::new(
            self.world_origin.x,
            self.world_origin.y,
            self.screen_size.w / self.zoom,
            self.screen_size.h / self.zoom,
        )
    }

    pub fn pan_world(&mut self, delta: Vec2) {
        self.world_origin += delta;
    }

    pub fn pan_screen(&mut self, delta: Vec2) {
        self.world_origin -= delta / self.zoom;
    }

    pub fn center_on(&mut self, world_point: Point) {
        self.world_origin = Point::new(
            world_point.x - (self.screen_size.w / self.zoom) / 2.0,
            world_point.y - (self.screen_size.h / self.zoom) / 2.0,
        );
    }

    pub fn zoom_at_screen(&mut self, anchor: Point, factor: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let anchored_world = self.screen_to_world(anchor);
        self.zoom = (self.zoom * factor).clamp(self.min_zoom, self.max_zoom);
        self.world_origin = Point::new(
            anchored_world.x - anchor.x / self.zoom,
            anchored_world.y - anchor.y / self.zoom,
        );
    }

    pub fn fit_rect(&mut self, rect: Rect, padding: f64) {
        let padded_w = (rect.size.w + padding * 2.0).max(1.0);
        let padded_h = (rect.size.h + padding * 2.0).max(1.0);
        let zoom_x = self.screen_size.w / padded_w;
        let zoom_y = self.screen_size.h / padded_h;
        self.zoom = zoom_x.min(zoom_y).clamp(self.min_zoom, self.max_zoom);

        let visible = self.visible_world_rect().size;
        let center = rect.center();
        self.world_origin = Point::new(center.x - visible.w / 2.0, center.y - visible.h / 2.0);
    }
}

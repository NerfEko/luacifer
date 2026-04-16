use crate::canvas::{Point, Rect, Size};

#[derive(Debug, Clone, PartialEq)]
pub struct OutputPlacement {
    pub name: String,
    pub origin: Point,
    pub size: Size,
}

impl OutputPlacement {
    pub fn rect(&self) -> Rect {
        Rect::new(self.origin.x, self.origin.y, self.size.w, self.size.h)
    }

    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.origin.x
            && point.y >= self.origin.y
            && point.x < self.origin.x + self.size.w
            && point.y < self.origin.y + self.size.h
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutputLayout {
    outputs: Vec<OutputPlacement>,
}

impl OutputLayout {
    pub fn add_output(&mut self, name: impl Into<String>, origin: Point, size: Size) {
        self.outputs.push(OutputPlacement {
            name: name.into(),
            origin,
            size,
        });
    }

    pub fn outputs(&self) -> &[OutputPlacement] {
        &self.outputs
    }

    pub fn output_at(&self, point: Point) -> Option<&OutputPlacement> {
        self.outputs.iter().find(|output| output.contains(point))
    }

    pub fn bounding_rect(&self) -> Option<Rect> {
        let first = self.outputs.first()?;
        let mut min_x = first.origin.x;
        let mut min_y = first.origin.y;
        let mut max_x = first.origin.x + first.size.w;
        let mut max_y = first.origin.y + first.size.h;

        for output in &self.outputs[1..] {
            min_x = min_x.min(output.origin.x);
            min_y = min_y.min(output.origin.y);
            max_x = max_x.max(output.origin.x + output.size.w);
            max_y = max_y.max(output.origin.y + output.size.h);
        }

        Some(Rect::new(min_x, min_y, max_x - min_x, max_y - min_y))
    }

    pub fn nearest_output(&self, point: Point) -> Option<&OutputPlacement> {
        self.outputs.iter().min_by(|a, b| {
            distance_squared(point, a.rect())
                .partial_cmp(&distance_squared(point, b.rect()))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }
}

fn distance_squared(point: Point, rect: Rect) -> f64 {
    let dx = if point.x < rect.origin.x {
        rect.origin.x - point.x
    } else if point.x > rect.origin.x + rect.size.w {
        point.x - (rect.origin.x + rect.size.w)
    } else {
        0.0
    };

    let dy = if point.y < rect.origin.y {
        rect.origin.y - point.y
    } else if point.y > rect.origin.y + rect.size.h {
        point.y - (rect.origin.y + rect.size.h)
    } else {
        0.0
    };

    dx * dx + dy * dy
}

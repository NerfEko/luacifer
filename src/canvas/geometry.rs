use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

impl Size {
    pub const fn new(w: f64, h: f64) -> Self {
        Self { w, h }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self {
            origin: Point::new(x, y),
            size: Size::new(w, h),
        }
    }

    pub fn center(self) -> Point {
        Point::new(
            self.origin.x + self.size.w / 2.0,
            self.origin.y + self.size.h / 2.0,
        )
    }
}

impl Add<Vec2> for Point {
    type Output = Point;

    fn add(self, rhs: Vec2) -> Self::Output {
        Point::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign<Vec2> for Point {
    fn add_assign(&mut self, rhs: Vec2) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl Sub<Vec2> for Point {
    type Output = Point;

    fn sub(self, rhs: Vec2) -> Self::Output {
        Point::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign<Vec2> for Point {
    fn sub_assign(&mut self, rhs: Vec2) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

impl Sub<Point> for Point {
    type Output = Vec2;

    fn sub(self, rhs: Point) -> Self::Output {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Add for Vec2 {
    type Output = Vec2;

    fn add(self, rhs: Vec2) -> Self::Output {
        Vec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;

    fn sub(self, rhs: Vec2) -> Self::Output {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Vec2;

    fn mul(self, rhs: f64) -> Self::Output {
        Vec2::new(self.x * rhs, self.y * rhs)
    }
}

impl Div<f64> for Vec2 {
    type Output = Vec2;

    fn div(self, rhs: f64) -> Self::Output {
        if rhs.abs() <= f64::EPSILON {
            return Vec2::new(0.0, 0.0);
        }
        Vec2::new(self.x / rhs, self.y / rhs)
    }
}

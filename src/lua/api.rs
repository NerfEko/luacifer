use crate::{canvas::Rect, window::WindowId};
use mlua::{Lua, Table, Value};

use super::ConfigError;

#[derive(Debug, Clone, PartialEq)]
pub struct WindowSnapshot {
    pub id: u64,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub bounds: Rect,
    pub floating: bool,
    pub exclude_from_focus: bool,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewportSnapshot {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
    pub screen_w: f64,
    pub screen_h: f64,
    pub visible_world: Rect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputSnapshot {
    pub id: String,
    pub logical_x: f64,
    pub logical_y: f64,
    pub viewport: ViewportSnapshot,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PointerSnapshot {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeStateSnapshot {
    pub focused_window_id: Option<u64>,
    pub pointer: PointerSnapshot,
    pub outputs: Vec<OutputSnapshot>,
    pub windows: Vec<WindowSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawSpace {
    Screen,
    World,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DrawCommand {
    Rect {
        space: DrawSpace,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        color: [f32; 4],
    },
    StrokeRect {
        space: DrawSpace,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        width: f64,
        outer: f64,
        color: [f32; 4],
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum HookAction {
    MoveWindow {
        id: u64,
        x: f64,
        y: f64,
    },
    ResizeWindow {
        id: u64,
        w: f64,
        h: f64,
    },
    SetBounds {
        id: u64,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    },
    FocusWindow {
        id: u64,
    },
    ClearFocus,
    CloseWindow {
        id: u64,
    },
    PanCanvas {
        dx: f64,
        dy: f64,
    },
    ZoomCanvas {
        factor: f64,
    },
}

impl DrawCommand {
    pub fn from_lua_table(table: Table) -> Result<Self, ConfigError> {
        let kind = table.get::<String>("kind")?;
        let space = parse_draw_space(table.get::<Option<String>>("space")?.as_deref())?;
        let x = table.get::<f64>("x")?;
        let y = table.get::<f64>("y")?;
        let w = table.get::<f64>("w")?;
        let h = table.get::<f64>("h")?;
        if w <= 0.0 || h <= 0.0 {
            return Err(ConfigError::Validation(
                "draw shapes require positive width and height".into(),
            ));
        }
        let color = parse_color(table.get::<Table>("color")?)?;

        match kind.as_str() {
            "rect" => Ok(Self::Rect {
                space,
                x,
                y,
                w,
                h,
                color,
            }),
            "stroke_rect" => {
                let width = table.get::<f64>("width")?;
                let outer = table.get::<Option<f64>>("outer")?.unwrap_or(0.0);
                if width <= 0.0 {
                    return Err(ConfigError::Validation(
                        "stroke_rect requires width > 0".into(),
                    ));
                }
                if outer < 0.0 {
                    return Err(ConfigError::Validation(
                        "stroke_rect outer must be >= 0".into(),
                    ));
                }
                Ok(Self::StrokeRect {
                    space,
                    x,
                    y,
                    w,
                    h,
                    width,
                    outer,
                    color,
                })
            }
            _ => Err(ConfigError::Validation(format!(
                "unsupported draw command kind: {kind}"
            ))),
        }
    }
}

impl HookAction {
    pub fn from_lua_table(table: Table) -> Result<Self, ConfigError> {
        let kind = table.get::<String>("kind")?;
        match kind.as_str() {
            "move_window" => Ok(Self::MoveWindow {
                id: table.get::<u64>("id")?,
                x: table.get::<f64>("x")?,
                y: table.get::<f64>("y")?,
            }),
            "resize_window" => Ok(Self::ResizeWindow {
                id: table.get::<u64>("id")?,
                w: table.get::<f64>("w")?,
                h: table.get::<f64>("h")?,
            }),
            "set_bounds" => Ok(Self::SetBounds {
                id: table.get::<u64>("id")?,
                x: table.get::<f64>("x")?,
                y: table.get::<f64>("y")?,
                w: table.get::<f64>("w")?,
                h: table.get::<f64>("h")?,
            }),
            "focus_window" => Ok(Self::FocusWindow {
                id: table.get::<u64>("id")?,
            }),
            "clear_focus" => Ok(Self::ClearFocus),
            "close_window" => Ok(Self::CloseWindow {
                id: table.get::<u64>("id")?,
            }),
            "pan_canvas" => Ok(Self::PanCanvas {
                dx: table.get::<f64>("dx")?,
                dy: table.get::<f64>("dy")?,
            }),
            "zoom_canvas" => Ok(Self::ZoomCanvas {
                factor: table.get::<f64>("factor")?,
            }),
            _ => Err(ConfigError::Validation(format!(
                "unsupported hook action kind: {kind}"
            ))),
        }
    }
}

pub fn parse_draw_commands(value: Value) -> Result<Vec<DrawCommand>, ConfigError> {
    match value {
        Value::Nil => Ok(Vec::new()),
        Value::Table(table) => {
            if let Some(shapes) = table.get::<Option<Table>>("shapes")? {
                let mut parsed = Vec::new();
                for shape in shapes.sequence_values::<Table>() {
                    parsed.push(DrawCommand::from_lua_table(shape?)?);
                }
                Ok(parsed)
            } else if table.get::<Option<String>>("kind")?.is_some() {
                Ok(vec![DrawCommand::from_lua_table(table)?])
            } else if table.raw_len() > 0 {
                let mut parsed = Vec::new();
                for shape in table.sequence_values::<Table>() {
                    parsed.push(DrawCommand::from_lua_table(shape?)?);
                }
                Ok(parsed)
            } else {
                Err(ConfigError::Validation(
                    "draw hook must return nil, a shape table, a sequence of shape tables, or { shapes = { ... } }".into(),
                ))
            }
        }
        _ => Err(ConfigError::Validation(
            "draw hook must return nil, a shape table, a sequence of shape tables, or { shapes = { ... } }".into(),
        )),
    }
}

pub fn register_draw_api(lua: &Lua, evil: &Table) -> Result<(), ConfigError> {
    let draw = lua.create_table()?;

    let rect = lua.create_function(|_, opts: Table| {
        opts.set("kind", "rect")?;
        Ok(opts)
    })?;
    draw.set("rect", rect)?;

    let stroke_rect = lua.create_function(|_, opts: Table| {
        opts.set("kind", "stroke_rect")?;
        Ok(opts)
    })?;
    draw.set("stroke_rect", stroke_rect)?;

    evil.set("draw", draw)?;
    Ok(())
}

fn parse_draw_space(space: Option<&str>) -> Result<DrawSpace, ConfigError> {
    match space.unwrap_or("world") {
        "world" => Ok(DrawSpace::World),
        "screen" => Ok(DrawSpace::Screen),
        other => Err(ConfigError::Validation(format!(
            "unsupported draw space: {other}"
        ))),
    }
}

fn parse_color(table: Table) -> Result<[f32; 4], ConfigError> {
    let values = [
        table.get::<f32>(1)?,
        table.get::<f32>(2)?,
        table.get::<f32>(3)?,
        table.get::<f32>(4)?,
    ];
    for value in values {
        if !(0.0..=1.0).contains(&value) {
            return Err(ConfigError::Validation(
                "draw colors must use normalized rgba values in the range 0..1".into(),
            ));
        }
    }
    Ok(values)
}

pub trait ActionTarget {
    fn move_window(&mut self, id: WindowId, x: f64, y: f64) -> bool;
    fn resize_window(&mut self, id: WindowId, w: f64, h: f64) -> bool;
    fn set_window_bounds(&mut self, id: WindowId, bounds: Rect) -> bool;
    fn focus_window(&mut self, id: WindowId) -> bool;
    fn clear_focus(&mut self) -> bool;
    fn close_window(&mut self, id: WindowId) -> bool;
    fn pan_canvas(&mut self, dx: f64, dy: f64);
    fn zoom_canvas(&mut self, factor: f64) -> Result<(), ConfigError>;
}

pub fn apply_hook_action<T: ActionTarget>(target: &mut T, action: HookAction) -> Result<(), ConfigError> {
    match action {
        HookAction::MoveWindow { id, x, y } => {
            let id = WindowId(id);
            if target.move_window(id, x, y) {
                Ok(())
            } else {
                Err(ConfigError::Validation(format!(
                    "hook action move_window failed for window id {}",
                    id.0
                )))
            }
        }
        HookAction::ResizeWindow { id, w, h } => {
            let id = WindowId(id);
            if target.resize_window(id, w, h) {
                Ok(())
            } else {
                Err(ConfigError::Validation(format!(
                    "hook action resize_window failed for window id {}",
                    id.0
                )))
            }
        }
        HookAction::SetBounds { id, x, y, w, h } => {
            let id = WindowId(id);
            if target.set_window_bounds(id, Rect::new(x, y, w, h)) {
                Ok(())
            } else {
                Err(ConfigError::Validation(format!(
                    "hook action set_bounds failed for window id {}",
                    id.0
                )))
            }
        }
        HookAction::FocusWindow { id } => {
            let id = WindowId(id);
            if target.focus_window(id) {
                Ok(())
            } else {
                Err(ConfigError::Validation(format!(
                    "hook action focus_window failed for window id {}",
                    id.0
                )))
            }
        }
        HookAction::ClearFocus => {
            target.clear_focus();
            Ok(())
        }
        HookAction::CloseWindow { id } => {
            let id = WindowId(id);
            if target.close_window(id) {
                Ok(())
            } else {
                Err(ConfigError::Validation(format!(
                    "hook action close_window failed for window id {}",
                    id.0
                )))
            }
        }
        HookAction::PanCanvas { dx, dy } => {
            target.pan_canvas(dx, dy);
            Ok(())
        }
        HookAction::ZoomCanvas { factor } => target.zoom_canvas(factor),
    }
}

pub fn parse_hook_actions(value: Value) -> Result<Vec<HookAction>, ConfigError> {
    match value {
        Value::Nil => Ok(Vec::new()),
        Value::Table(table) => {
            if let Some(actions) = table.get::<Option<Table>>("actions")? {
                let mut parsed = Vec::new();
                for action in actions.sequence_values::<Table>() {
                    parsed.push(HookAction::from_lua_table(action?)?);
                }
                Ok(parsed)
            } else {
                Ok(vec![HookAction::from_lua_table(table)?])
            }
        }
        _ => Err(ConfigError::Validation(
            "hook must return nil, an action table, or { actions = { ... } }".into(),
        )),
    }
}

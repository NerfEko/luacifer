use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use mlua::{Lua, Table, Value};

use crate::input::bindings::canonical_modifier_name;
use crate::input::parse_keyspec;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub backend: Option<String>,
    pub canvas: CanvasConfig,
    pub draw: DrawConfig,
    pub window: WindowConfig,
    pub placement: PlacementConfig,
    pub tty: TtyConfig,
    pub autostart: Vec<String>,
    pub bindings: Vec<BindingConfig>,
    pub rules: Vec<RuleConfig>,
    pub source_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanvasConfig {
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub zoom_step: f64,
    pub pan_step: f64,
    pub allow_pointer_zoom: bool,
    pub allow_middle_click_pan: bool,
    pub allow_gesture_navigation: bool,
}

impl Default for CanvasConfig {
    fn default() -> Self {
        Self {
            min_zoom: 0.1,
            max_zoom: 8.0,
            zoom_step: 1.2,
            pan_step: 64.0,
            allow_pointer_zoom: true,
            allow_middle_click_pan: true,
            allow_gesture_navigation: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawLayer {
    Background,
    Windows,
    WindowOverlay,
    Popups,
    Overlay,
    Cursor,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DrawConfig {
    /// User-facing stacking order from bottom-most layer to top-most layer.
    pub stack: Vec<DrawLayer>,
    /// Clear color used before compositing draw/background/window layers.
    pub clear_color: [f32; 4],
}

impl Default for DrawConfig {
    fn default() -> Self {
        Self {
            stack: vec![
                DrawLayer::Background,
                DrawLayer::Windows,
                DrawLayer::WindowOverlay,
                DrawLayer::Popups,
                DrawLayer::Overlay,
                DrawLayer::Cursor,
            ],
            clear_color: [0.08, 0.05, 0.12, 1.0],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowConfig {
    pub use_client_default_size: bool,
    pub remember_sizes_by_app_id: bool,
    pub hide_client_decorations: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacementConfig {
    pub default_size: (f64, f64),
    pub padding: f64,
    pub cascade_step: (f64, f64),
}

impl Default for PlacementConfig {
    fn default() -> Self {
        Self {
            default_size: (900.0, 600.0),
            padding: 32.0,
            cascade_step: (32.0, 24.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtyOutputLayout {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TtyConfig {
    pub quit_mods: Vec<String>,
    pub quit_key: String,
    pub vt_switch_modifiers: Vec<String>,
    pub output_layout: TtyOutputLayout,
}

impl Default for TtyConfig {
    fn default() -> Self {
        Self {
            quit_mods: vec!["Ctrl".into(), "Alt".into()],
            quit_key: "Backspace".into(),
            vt_switch_modifiers: vec!["Ctrl".into(), "Alt".into()],
            output_layout: TtyOutputLayout::Horizontal,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            use_client_default_size: true,
            remember_sizes_by_app_id: true,
            hide_client_decorations: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BindingConfig {
    pub mods: Vec<String>,
    pub key: String,
    pub action: String,
    pub amount: Option<f64>,
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleConfig {
    pub app_id: Option<String>,
    pub title_contains: Option<String>,
    pub floating: Option<bool>,
    pub exclude_from_focus: Option<bool>,
    pub width: Option<f64>,
    pub height: Option<f64>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Lua(mlua::Error),
    Validation(String),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ConfigBuilder {
    backend: Option<String>,
    canvas: CanvasConfig,
    draw: DrawConfig,
    window: WindowConfig,
    placement: PlacementConfig,
    tty: TtyConfig,
    autostart: Vec<String>,
    bindings: Vec<BindingConfig>,
    rules: Vec<RuleConfig>,
    used_script_api: bool,
}

pub(crate) fn resolve_include_path(
    base_dir: &Path,
    relative_path: &Path,
) -> Result<PathBuf, ConfigError> {
    if relative_path.is_absolute() {
        return Err(ConfigError::Validation(
            "include() only accepts relative paths inside the config root".into(),
        ));
    }

    let canonical_base = fs::canonicalize(base_dir).map_err(|source| ConfigError::Io {
        path: base_dir.to_path_buf(),
        source,
    })?;
    let requested_path = canonical_base.join(relative_path);
    let canonical_path = fs::canonicalize(&requested_path).map_err(|source| ConfigError::Io {
        path: requested_path.clone(),
        source,
    })?;

    if !canonical_path.starts_with(&canonical_base) {
        return Err(ConfigError::Validation(format!(
            "include() path escapes the config root: {}",
            relative_path.display()
        )));
    }

    Ok(canonical_path)
}

pub(crate) fn register_root_include(lua: &Lua, base_dir: PathBuf) -> Result<(), ConfigError> {
    let include = lua.create_function(move |lua, relative_path: String| {
        let full_path = resolve_include_path(&base_dir, Path::new(&relative_path))
            .map_err(mlua::Error::external)?;
        let source = fs::read_to_string(&full_path).map_err(mlua::Error::external)?;
        lua.load(&source)
            .set_name(full_path.to_string_lossy().as_ref())
            .eval::<Value>()
    })?;
    lua.globals().set("include", include)?;
    Ok(())
}

impl Config {
    pub fn from_lua_value(value: Value, source_root: &Path) -> Result<Self, ConfigError> {
        let table = match value {
            Value::Table(table) => table,
            _ => {
                return Err(ConfigError::Validation(
                    "config root must return a table".into(),
                ));
            }
        };

        let backend = table.get::<Option<String>>("backend")?;
        let autostart = parse_string_list(table.get::<Option<Table>>("autostart")?)?;
        let bindings = parse_bindings(table.get::<Option<Table>>("bindings")?)?;
        let rules = parse_rules(table.get::<Option<Table>>("rules")?)?;

        let canvas = if let Some(canvas_table) = table.get::<Option<Table>>("canvas")? {
            parse_canvas_table(&canvas_table, CanvasConfig::default())?
        } else {
            CanvasConfig::default()
        };
        let draw = if let Some(draw_table) = table.get::<Option<Table>>("draw")? {
            parse_draw_table(&draw_table, DrawConfig::default())?
        } else {
            DrawConfig::default()
        };
        let window = if let Some(window_table) = table.get::<Option<Table>>("window")? {
            parse_window_table(&window_table, WindowConfig::default())?
        } else {
            WindowConfig::default()
        };

        let placement = if let Some(placement_table) = table.get::<Option<Table>>("placement")? {
            parse_placement_table(&placement_table, PlacementConfig::default())?
        } else {
            PlacementConfig::default()
        };
        let tty = if let Some(tty_table) = table.get::<Option<Table>>("tty")? {
            parse_tty_table(&tty_table, TtyConfig::default())?
        } else {
            TtyConfig::default()
        };

        let config = Self {
            backend,
            canvas,
            draw,
            window,
            placement,
            tty,
            autostart,
            bindings,
            rules,
            source_root: source_root.to_path_buf(),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if let Some(backend) = self.backend.as_deref()
            && !matches!(backend, "winit" | "udev" | "headless")
        {
            return Err(ConfigError::Validation(format!(
                "unsupported backend in config: {backend}"
            )));
        }

        if !self.canvas.min_zoom.is_finite() {
            return Err(ConfigError::Validation(
                "canvas.min_zoom must be a finite number".into(),
            ));
        }
        if self.canvas.min_zoom <= 0.0 {
            return Err(ConfigError::Validation(
                "canvas.min_zoom must be > 0".into(),
            ));
        }
        if !self.canvas.max_zoom.is_finite() {
            return Err(ConfigError::Validation(
                "canvas.max_zoom must be a finite number".into(),
            ));
        }
        if self.canvas.max_zoom < self.canvas.min_zoom {
            return Err(ConfigError::Validation(
                "canvas.max_zoom must be >= canvas.min_zoom".into(),
            ));
        }
        if !self.canvas.zoom_step.is_finite() {
            return Err(ConfigError::Validation(
                "canvas.zoom_step must be a finite number".into(),
            ));
        }
        if self.canvas.zoom_step <= 0.0 {
            return Err(ConfigError::Validation(
                "canvas.zoom_step must be > 0".into(),
            ));
        }
        if !self.canvas.pan_step.is_finite() {
            return Err(ConfigError::Validation(
                "canvas.pan_step must be a finite number".into(),
            ));
        }
        if self.canvas.pan_step < 0.0 {
            return Err(ConfigError::Validation(
                "canvas.pan_step must be >= 0".into(),
            ));
        }

        validate_draw_stack(&self.draw.stack)?;
        validate_color(&self.draw.clear_color, "draw.clear_color")?;
        validate_placement(&self.placement)?;
        validate_tty(&self.tty)?;

        const SUPPORTED_ACTIONS: &[&str] = &[
            "pan_left",
            "pan_right",
            "pan_up",
            "pan_down",
            "zoom_in",
            "zoom_out",
            "close_window",
            "spawn",
            "focus_next",
            "focus_prev",
            "quit",
        ];

        for binding in &self.bindings {
            if binding.key.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "binding key must not be empty".into(),
                ));
            }
            for modifier in &binding.mods {
                if canonical_modifier_name(modifier).is_none() {
                    return Err(ConfigError::Validation(format!(
                        "unsupported modifier in binding config: {modifier}"
                    )));
                }
            }
            if binding.action.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "binding action must not be empty".into(),
                ));
            }
            if !SUPPORTED_ACTIONS.contains(&binding.action.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "unsupported binding action: {}",
                    binding.action
                )));
            }
            if binding.action == "spawn"
                && binding
                    .command
                    .as_ref()
                    .is_none_or(|command| command.trim().is_empty())
            {
                return Err(ConfigError::Validation(
                    "spawn bindings must include a non-empty command".into(),
                ));
            }
            if matches!(binding.amount, Some(amount) if amount <= 0.0) {
                return Err(ConfigError::Validation(
                    "binding amount must be > 0 when provided".into(),
                ));
            }
        }

        for command in &self.autostart {
            if command.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "autostart commands must not be empty".into(),
                ));
            }
        }

        for rule in &self.rules {
            if rule.app_id.is_none() && rule.title_contains.is_none() {
                return Err(ConfigError::Validation(
                    "window rule must match at least one field".into(),
                ));
            }
            if matches!(rule.width, Some(width) if width <= 0.0) {
                return Err(ConfigError::Validation("rule width must be > 0".into()));
            }
            if matches!(rule.height, Some(height) if height <= 0.0) {
                return Err(ConfigError::Validation("rule height must be > 0".into()));
            }
        }

        Ok(())
    }
}

impl ConfigBuilder {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn uses_script_api(&self) -> bool {
        self.used_script_api
    }

    pub fn apply_config_table(&mut self, table: Table) -> Result<(), ConfigError> {
        self.used_script_api = true;

        if let Some(backend) = table.get::<Option<String>>("backend")? {
            self.backend = Some(backend);
        }

        if let Some(canvas_table) = table.get::<Option<Table>>("canvas")? {
            self.canvas = parse_canvas_table(&canvas_table, self.canvas.clone())?;
        }
        if let Some(draw_table) = table.get::<Option<Table>>("draw")? {
            self.draw = parse_draw_table(&draw_table, self.draw.clone())?;
        }
        if let Some(window_table) = table.get::<Option<Table>>("window")? {
            self.window = parse_window_table(&window_table, self.window.clone())?;
        }
        if let Some(placement_table) = table.get::<Option<Table>>("placement")? {
            self.placement = parse_placement_table(&placement_table, self.placement.clone())?;
        }
        if let Some(tty_table) = table.get::<Option<Table>>("tty")? {
            self.tty = parse_tty_table(&tty_table, self.tty.clone())?;
        }

        self.autostart
            .extend(parse_string_list(table.get::<Option<Table>>("autostart")?)?);
        self.bindings
            .extend(parse_bindings(table.get::<Option<Table>>("bindings")?)?);
        self.rules
            .extend(parse_rules(table.get::<Option<Table>>("rules")?)?);

        Ok(())
    }

    pub fn add_binding(
        &mut self,
        keyspec: &str,
        action: &str,
        options: Option<Table>,
    ) -> Result<(), ConfigError> {
        let (mods, key) = parse_keyspec(keyspec).map_err(ConfigError::Validation)?;
        let amount = options
            .as_ref()
            .map(|table| table.get::<Option<f64>>("amount"))
            .transpose()?
            .flatten();
        let command = options
            .as_ref()
            .map(|table| table.get::<Option<String>>("command"))
            .transpose()?
            .flatten();

        self.used_script_api = true;
        self.bindings.push(BindingConfig {
            mods,
            key,
            action: action.to_string(),
            amount,
            command,
        });
        Ok(())
    }

    pub fn add_autostart(&mut self, command: &str) -> Result<(), ConfigError> {
        if command.trim().is_empty() {
            return Err(ConfigError::Validation(
                "autostart command must not be empty".into(),
            ));
        }

        self.used_script_api = true;
        self.autostart.push(command.to_string());
        Ok(())
    }

    pub fn build(&self, source_root: &Path) -> Result<Config, ConfigError> {
        let config = Config {
            backend: self.backend.clone(),
            canvas: self.canvas.clone(),
            draw: self.draw.clone(),
            window: self.window.clone(),
            placement: self.placement.clone(),
            tty: self.tty.clone(),
            autostart: self.autostart.clone(),
            bindings: self.bindings.clone(),
            rules: self.rules.clone(),
            source_root: source_root.to_path_buf(),
        };
        config.validate()?;
        Ok(config)
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "failed to read {}: {}", path.display(), source),
            Self::Lua(error) => write!(f, "lua error: {error}"),
            Self::Validation(message) => write!(f, "config validation error: {message}"),
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Lua(error) => Some(error),
            Self::Validation(_) => None,
        }
    }
}

impl From<mlua::Error> for ConfigError {
    fn from(value: mlua::Error) -> Self {
        Self::Lua(value)
    }
}

fn parse_canvas_table(table: &Table, base: CanvasConfig) -> Result<CanvasConfig, ConfigError> {
    Ok(CanvasConfig {
        min_zoom: table
            .get::<Option<f64>>("min_zoom")?
            .unwrap_or(base.min_zoom),
        max_zoom: table
            .get::<Option<f64>>("max_zoom")?
            .unwrap_or(base.max_zoom),
        zoom_step: table
            .get::<Option<f64>>("zoom_step")?
            .unwrap_or(base.zoom_step),
        pan_step: table
            .get::<Option<f64>>("pan_step")?
            .unwrap_or(base.pan_step),
        allow_pointer_zoom: table
            .get::<Option<bool>>("allow_pointer_zoom")?
            .unwrap_or(base.allow_pointer_zoom),
        allow_middle_click_pan: table
            .get::<Option<bool>>("allow_middle_click_pan")?
            .unwrap_or(base.allow_middle_click_pan),
        allow_gesture_navigation: table
            .get::<Option<bool>>("allow_gesture_navigation")?
            .unwrap_or(base.allow_gesture_navigation),
    })
}

fn parse_draw_table(table: &Table, base: DrawConfig) -> Result<DrawConfig, ConfigError> {
    let stack = match table.get::<Option<Table>>("stack")? {
        Some(stack_table) => parse_draw_stack(&stack_table)?,
        None => base.stack,
    };
    let clear_color = match table.get::<Option<Table>>("clear_color")? {
        Some(color_table) => parse_color_table(&color_table, "draw.clear_color")?,
        None => base.clear_color,
    };
    Ok(DrawConfig { stack, clear_color })
}

fn parse_window_table(table: &Table, base: WindowConfig) -> Result<WindowConfig, ConfigError> {
    Ok(WindowConfig {
        use_client_default_size: table
            .get::<Option<bool>>("use_client_default_size")?
            .unwrap_or(base.use_client_default_size),
        remember_sizes_by_app_id: table
            .get::<Option<bool>>("remember_sizes_by_app_id")?
            .unwrap_or(base.remember_sizes_by_app_id),
        hide_client_decorations: table
            .get::<Option<bool>>("hide_client_decorations")?
            .unwrap_or(base.hide_client_decorations),
    })
}

fn parse_placement_table(
    table: &Table,
    base: PlacementConfig,
) -> Result<PlacementConfig, ConfigError> {
    let default_size = match table.get::<Option<Table>>("default_size")? {
        Some(size_table) => (
            size_table
                .get::<Option<f64>>("w")?
                .unwrap_or(base.default_size.0),
            size_table
                .get::<Option<f64>>("h")?
                .unwrap_or(base.default_size.1),
        ),
        None => base.default_size,
    };
    let cascade_step = match table.get::<Option<Table>>("cascade_step")? {
        Some(step_table) => (
            step_table
                .get::<Option<f64>>("x")?
                .unwrap_or(base.cascade_step.0),
            step_table
                .get::<Option<f64>>("y")?
                .unwrap_or(base.cascade_step.1),
        ),
        None => base.cascade_step,
    };

    Ok(PlacementConfig {
        default_size,
        padding: table.get::<Option<f64>>("padding")?.unwrap_or(base.padding),
        cascade_step,
    })
}

fn parse_tty_table(table: &Table, base: TtyConfig) -> Result<TtyConfig, ConfigError> {
    let (quit_mods, quit_key) =
        if let Some(keyspec) = table.get::<Option<String>>("quit_keyspec")? {
            parse_keyspec(&keyspec).map_err(ConfigError::Validation)?
        } else {
            (base.quit_mods, base.quit_key)
        };

    let vt_switch_modifiers = match table.get::<Option<Table>>("vt_switch_modifiers")? {
        Some(mods_table) => parse_modifier_name_list(&mods_table, "tty.vt_switch_modifiers")?,
        None => base.vt_switch_modifiers,
    };
    let output_layout = match table.get::<Option<String>>("output_layout")?.as_deref() {
        Some("horizontal") => TtyOutputLayout::Horizontal,
        Some("vertical") => TtyOutputLayout::Vertical,
        Some(other) => {
            return Err(ConfigError::Validation(format!(
                "unsupported tty.output_layout: {other}"
            )));
        }
        None => base.output_layout,
    };

    Ok(TtyConfig {
        quit_mods,
        quit_key,
        vt_switch_modifiers,
        output_layout,
    })
}

fn parse_draw_stack(table: &Table) -> Result<Vec<DrawLayer>, ConfigError> {
    let mut stack = Vec::new();
    for item in table.sequence_values::<String>() {
        stack.push(parse_draw_layer_name(&item?)?);
    }
    Ok(stack)
}

fn parse_draw_layer_name(name: &str) -> Result<DrawLayer, ConfigError> {
    match name {
        "background" => Ok(DrawLayer::Background),
        "windows" => Ok(DrawLayer::Windows),
        "window_overlay" => Ok(DrawLayer::WindowOverlay),
        "popups" => Ok(DrawLayer::Popups),
        "overlay" => Ok(DrawLayer::Overlay),
        "cursor" => Ok(DrawLayer::Cursor),
        _ => Err(ConfigError::Validation(format!(
            "unsupported draw layer in draw.stack: {name}"
        ))),
    }
}

fn validate_draw_stack(stack: &[DrawLayer]) -> Result<(), ConfigError> {
    use DrawLayer::{Background, Cursor, Overlay, Popups, WindowOverlay, Windows};

    if stack.len() != 6 {
        return Err(ConfigError::Validation(
            "draw.stack must list exactly 6 layers: background, windows, window_overlay, popups, overlay, cursor".into(),
        ));
    }

    for required in [Background, Windows, WindowOverlay, Popups, Overlay, Cursor] {
        let count = stack.iter().filter(|layer| **layer == required).count();
        if count != 1 {
            return Err(ConfigError::Validation(
                "draw.stack must include each layer exactly once: background, windows, window_overlay, popups, overlay, cursor"
                    .into(),
            ));
        }
    }

    Ok(())
}

fn parse_color_table(table: &Table, field_name: &str) -> Result<[f32; 4], ConfigError> {
    let mut color = [0.0_f32; 4];
    for (index, slot) in color.iter_mut().enumerate() {
        let component = table.get::<f64>((index + 1) as i64).map_err(|_| {
            ConfigError::Validation(format!(
                "{field_name} must contain exactly 4 numeric components"
            ))
        })?;
        if !component.is_finite() {
            return Err(ConfigError::Validation(format!(
                "{field_name} components must be finite"
            )));
        }
        *slot = component as f32;
    }
    Ok(color)
}

fn parse_modifier_name_list(table: &Table, field_name: &str) -> Result<Vec<String>, ConfigError> {
    let mut modifiers = Vec::new();
    for item in table.sequence_values::<String>() {
        let name = item?;
        let canonical = canonical_modifier_name(&name).ok_or_else(|| {
            ConfigError::Validation(format!("unsupported modifier in {field_name}: {name}"))
        })?;
        modifiers.push(canonical.to_string());
    }
    Ok(modifiers)
}

fn validate_color(color: &[f32; 4], field_name: &str) -> Result<(), ConfigError> {
    if color.iter().any(|component| !component.is_finite()) {
        return Err(ConfigError::Validation(format!(
            "{field_name} must contain finite components"
        )));
    }
    Ok(())
}

fn validate_placement(placement: &PlacementConfig) -> Result<(), ConfigError> {
    if !placement.default_size.0.is_finite() || placement.default_size.0 <= 0.0 {
        return Err(ConfigError::Validation(
            "placement.default_size.w must be a positive finite number".into(),
        ));
    }
    if !placement.default_size.1.is_finite() || placement.default_size.1 <= 0.0 {
        return Err(ConfigError::Validation(
            "placement.default_size.h must be a positive finite number".into(),
        ));
    }
    if !placement.padding.is_finite() || placement.padding < 0.0 {
        return Err(ConfigError::Validation(
            "placement.padding must be a finite number >= 0".into(),
        ));
    }
    if !placement.cascade_step.0.is_finite() || !placement.cascade_step.1.is_finite() {
        return Err(ConfigError::Validation(
            "placement.cascade_step must use finite numbers".into(),
        ));
    }
    Ok(())
}

fn validate_tty(tty: &TtyConfig) -> Result<(), ConfigError> {
    if tty.quit_key.trim().is_empty() {
        return Err(ConfigError::Validation(
            "tty.quit_keyspec must not be empty".into(),
        ));
    }
    if tty.vt_switch_modifiers.is_empty() {
        return Err(ConfigError::Validation(
            "tty.vt_switch_modifiers must not be empty".into(),
        ));
    }
    Ok(())
}

fn parse_string_list(table: Option<Table>) -> Result<Vec<String>, ConfigError> {
    let mut values = Vec::new();

    if let Some(table) = table {
        for item in table.sequence_values::<String>() {
            values.push(item?);
        }
    }

    Ok(values)
}

fn parse_bindings(table: Option<Table>) -> Result<Vec<BindingConfig>, ConfigError> {
    let mut bindings = Vec::new();

    if let Some(table) = table {
        for entry in table.sequence_values::<Table>() {
            let entry = entry?;
            bindings.push(BindingConfig {
                mods: parse_string_list(entry.get::<Option<Table>>("mods")?)?,
                key: entry.get::<String>("key")?,
                action: entry.get::<String>("action")?,
                amount: entry.get::<Option<f64>>("amount")?,
                command: entry.get::<Option<String>>("command")?,
            });
        }
    }

    Ok(bindings)
}

fn parse_rules(table: Option<Table>) -> Result<Vec<RuleConfig>, ConfigError> {
    let mut rules = Vec::new();

    if let Some(table) = table {
        for entry in table.sequence_values::<Table>() {
            let entry = entry?;
            let size = entry.get::<Option<Table>>("size")?;
            rules.push(RuleConfig {
                app_id: entry.get::<Option<String>>("app_id")?,
                title_contains: entry.get::<Option<String>>("title_contains")?,
                floating: entry.get::<Option<bool>>("floating")?,
                exclude_from_focus: entry.get::<Option<bool>>("exclude_from_focus")?,
                width: size.as_ref().map(|size| size.get::<f64>("w")).transpose()?,
                height: size.as_ref().map(|size| size.get::<f64>("h")).transpose()?,
            });
        }
    }

    Ok(rules)
}

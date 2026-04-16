use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use mlua::{Table, Value};

use crate::input::parse_keyspec;
use crate::input::bindings::canonical_modifier_name;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub backend: Option<String>,
    pub canvas: CanvasConfig,
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
}

impl Default for CanvasConfig {
    fn default() -> Self {
        Self {
            min_zoom: 0.1,
            max_zoom: 8.0,
            zoom_step: 1.2,
            pan_step: 64.0,
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
    autostart: Vec<String>,
    bindings: Vec<BindingConfig>,
    rules: Vec<RuleConfig>,
    used_script_api: bool,
}

pub(crate) fn resolve_include_path(base_dir: &Path, relative_path: &Path) -> Result<PathBuf, ConfigError> {
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

        let config = Self {
            backend,
            canvas,
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
            && !matches!(backend, "x11" | "winit" | "udev" | "headless")
        {
            return Err(ConfigError::Validation(format!(
                "unsupported backend in config: {backend}"
            )));
        }

        if self.canvas.min_zoom <= 0.0 {
            return Err(ConfigError::Validation(
                "canvas.min_zoom must be > 0".into(),
            ));
        }
        if self.canvas.max_zoom < self.canvas.min_zoom {
            return Err(ConfigError::Validation(
                "canvas.max_zoom must be >= canvas.min_zoom".into(),
            ));
        }
        if self.canvas.zoom_step <= 0.0 {
            return Err(ConfigError::Validation(
                "canvas.zoom_step must be > 0".into(),
            ));
        }
        if self.canvas.pan_step < 0.0 {
            return Err(ConfigError::Validation(
                "canvas.pan_step must be >= 0".into(),
            ));
        }

        const SUPPORTED_ACTIONS: &[&str] = &[
            "pan_left",
            "pan_right",
            "pan_up",
            "pan_down",
            "zoom_in",
            "zoom_out",
            "close_window",
            "spawn",
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
    })
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

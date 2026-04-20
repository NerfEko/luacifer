use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use mlua::{Lua, Table, Value};

use crate::lua::{
    config::{Config, ConfigBuilder, ConfigError, register_root_include},
    register_draw_api,
};

#[derive(Debug)]
pub struct LuaRuntime {
    lua: Lua,
    base_dir: PathBuf,
    builder: Rc<RefCell<ConfigBuilder>>,
}

impl LuaRuntime {
    pub fn new(base_dir: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let lua = Lua::new();
        let base_dir = base_dir.into();
        let builder = Rc::new(RefCell::new(ConfigBuilder::default()));

        register_root_include(&lua, base_dir.clone())?;
        register_evil_api(&lua, &builder)?;

        Ok(Self {
            lua,
            base_dir,
            builder,
        })
    }

    pub fn load_config_file(&self, path: impl AsRef<Path>) -> Result<Config, ConfigError> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        self.load_config_str(&source, path)
    }

    pub fn load_config_str(
        &self,
        source: &str,
        name: impl AsRef<Path>,
    ) -> Result<Config, ConfigError> {
        self.builder.borrow_mut().clear();

        let name = name.as_ref();
        let value = self
            .lua
            .load(source)
            .set_name(name.to_string_lossy().as_ref())
            .eval::<Value>()?;

        let mut builder = self.builder.borrow_mut();
        if builder.uses_script_api() {
            if let Value::Table(table) = value {
                builder.apply_config_table(table)?;
            }
            return builder.build(&self.base_dir);
        }

        Config::from_lua_value(value, &self.base_dir)
    }
}

fn register_evil_api(lua: &Lua, builder: &Rc<RefCell<ConfigBuilder>>) -> Result<(), ConfigError> {
    let evil = lua.create_table()?;

    let config_builder = builder.clone();
    let config = lua.create_function(move |_, table: Table| {
        config_builder
            .borrow_mut()
            .apply_config_table(table)
            .map_err(mlua::Error::external)
    })?;
    evil.set("config", config)?;

    let bind_builder = builder.clone();
    let bind = lua.create_function(
        move |_, (keyspec, action, options): (String, String, Option<Table>)| {
            bind_builder
                .borrow_mut()
                .add_binding(&keyspec, &action, options)
                .map_err(mlua::Error::external)
        },
    )?;
    evil.set("bind", bind)?;

    let autostart_builder = builder.clone();
    let autostart = lua.create_function(move |_, command: String| {
        autostart_builder
            .borrow_mut()
            .add_autostart(&command)
            .map_err(mlua::Error::external)
    })?;
    evil.set("autostart", autostart)?;

    evil.set("on", lua.create_table()?)?;
    evil.set("window", lua.create_table()?)?;
    evil.set("canvas", lua.create_table()?)?;
    evil.set("output", lua.create_table()?)?;
    evil.set("pointer", lua.create_table()?)?;
    register_draw_api(lua, &evil)?;

    lua.globals().set("evil", evil)?;
    Ok(())
}

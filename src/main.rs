use std::path::PathBuf;

#[cfg(feature = "lua")]
use std::path::Path;

use clap::{Parser, ValueEnum};

use evilwm::headless::{HeadlessOptions, run_headless};
use evilwm::ipc::RuntimeSnapshot;
#[cfg(feature = "udev")]
use evilwm::compositor::run_udev;
#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
use evilwm::compositor::{RuntimeOptions, run_winit};
#[cfg(feature = "lua")]
use evilwm::lua::LuaRuntime;

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum Backend {
    X11,
    Winit,
    Udev,
    Headless,
}

impl Backend {
    fn from_config_name(name: &str) -> Option<Self> {
        match name {
            "x11" => Some(Self::X11),
            "winit" => Some(Self::Winit),
            "udev" => Some(Self::Udev),
            "headless" => Some(Self::Headless),
            _ => None,
        }
    }
}

#[derive(Debug, Parser)]
#[command(author, version, about = "evilwm compositor prototype")]
struct Cli {
    #[arg(long, value_enum)]
    backend: Option<Backend>,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    check_config: bool,
    #[arg(long)]
    no_config: bool,
    #[arg(long)]
    command: Option<String>,
    #[arg(long)]
    dump_state_json: bool,
}

fn main() {
    init_logging();

    let cli = Cli::parse();

    if cli.check_config {
        run_config_check(&cli);
        return;
    }

    run_mode(cli);
}

fn resolve_backend_choice(cli_backend: Option<Backend>, config_backend: Option<&str>) -> Backend {
    cli_backend
        .or_else(|| config_backend.and_then(Backend::from_config_name))
        .unwrap_or(Backend::Winit)
}

fn init_logging() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }
}

#[cfg(feature = "lua")]
fn resolve_config_path(explicit: Option<&Path>) -> Option<PathBuf> {
    explicit.map(Path::to_path_buf).or_else(default_config_path)
}

#[cfg(feature = "lua")]
fn default_config_path() -> Option<PathBuf> {
    if let Ok(xdg_config_home) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg_config_home).join("evilwm/config.lua"));
    }

    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config/evilwm/config.lua"))
}

#[cfg(feature = "lua")]
fn load_config(cli: &Cli) -> (Option<PathBuf>, Option<evilwm::lua::Config>) {
    if cli.no_config {
        return (None, None);
    }

    let resolved = resolve_config_path(cli.config.as_deref());
    if let Some(explicit) = cli.config.as_deref()
        && !explicit.exists()
    {
        report_fatal(format!("config missing: {}", explicit.display()));
    }

    let loaded = resolved
        .as_deref()
        .filter(|path| path.exists())
        .map(|path| {
            let runtime = LuaRuntime::new(
                path.parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(".")),
            )?;
            runtime.load_config_file(path)
        })
        .transpose()
        .unwrap_or_else(|error| report_fatal(error));

    (resolved, loaded)
}

#[cfg(feature = "lua")]
fn run_config_check(cli: &Cli) {
    if cli.no_config {
        println!("config check skipped: --no-config set");
        return;
    }

    let Some(config_path) = resolve_config_path(cli.config.as_deref()) else {
        eprintln!("unable to resolve config path");
        std::process::exit(2);
    };

    let runtime = LuaRuntime::new(
        config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    )
    .unwrap_or_else(|error| report_fatal(error));

    let config = runtime
        .load_config_file(&config_path)
        .unwrap_or_else(|error| report_fatal(error));

    println!(
        "config OK: {} (bindings={}, rules={}, autostart={})",
        config_path.display(),
        config.bindings.len(),
        config.rules.len(),
        config.autostart.len()
    );
}

#[cfg(not(feature = "lua"))]
fn run_config_check(cli: &Cli) {
    if cli.no_config {
        println!("config check skipped: --no-config set");
        return;
    }

    let _ = cli;
    eprintln!("config support is disabled; rebuild with --features lua");
    std::process::exit(2);
}

fn run_mode(cli: Cli) {
    #[cfg(feature = "lua")]
    let (config_path, config) = load_config(&cli);
    #[cfg(not(feature = "lua"))]
    let (config_path, config) = (None, None);

    let backend = resolve_backend_choice(
        cli.backend,
        #[cfg(feature = "lua")]
        config.as_ref().and_then(|config| config.backend.as_deref()),
        #[cfg(not(feature = "lua"))]
        None,
    );

    match backend {
        Backend::Headless => {
            let session = run_headless(HeadlessOptions {
                config_path,
                config,
                ..HeadlessOptions::default()
            });
            if cli.dump_state_json {
                let snapshot = RuntimeSnapshot::from_headless(&session);
                println!(
                    "{}",
                    snapshot
                        .to_json_pretty()
                        .unwrap_or_else(|error| report_fatal(error))
                );
            } else {
                println!("{}", session.report());
            }
        }
        Backend::Winit | Backend::X11 => {
            #[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
            {
                if matches!(backend, Backend::X11) {
                    eprintln!(
                        "x11 backend is not implemented yet; starting the nested winit backend instead"
                    );
                }
                let options = RuntimeOptions {
                    command: cli.command,
                    config_path,
                    config,
                };
                run_winit(options).unwrap_or_else(|error| report_fatal(error));
            }
            #[cfg(not(any(feature = "winit", feature = "x11", feature = "udev")))]
            {
                let _ = (config_path, config);
                eprintln!(
                    "live backend support is disabled in this build; rebuild with --features winit or udev"
                );
                std::process::exit(2);
            }
        }
        Backend::Udev => {
            #[cfg(feature = "udev")]
            {
                let options = RuntimeOptions {
                    command: cli.command,
                    config_path,
                    config,
                };
                run_udev(options).unwrap_or_else(|error| report_fatal(error));
            }
            #[cfg(not(feature = "udev"))]
            {
                let _ = (config_path, config);
                eprintln!(
                    "udev backend support is disabled in this build; rebuild with --features udev"
                );
                std::process::exit(2);
            }
        }
    }
}

fn report_fatal(error: impl std::fmt::Display) -> ! {
    eprintln!("{error}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::{Backend, resolve_backend_choice};

    #[test]
    fn cli_backend_overrides_config_backend() {
        assert_eq!(
            resolve_backend_choice(Some(Backend::Headless), Some("udev")),
            Backend::Headless
        );
    }

    #[test]
    fn config_backend_is_used_when_cli_backend_is_omitted() {
        assert_eq!(
            resolve_backend_choice(None, Some("udev")),
            Backend::Udev
        );
    }

    #[test]
    fn backend_defaults_to_winit_when_nothing_is_specified() {
        assert_eq!(resolve_backend_choice(None, None), Backend::Winit);
    }
}

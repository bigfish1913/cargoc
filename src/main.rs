use cargoc::{
    Result,
    config::{Config, default_config_path, legacy_config_path, migrate_legacy_config},
    fail, integration, runner, self_update,
    store::{GcOptions, Store},
    util::{GIB, absolute, same_or_descendant},
};
use clap::{Args, Parser, Subcommand};
use serde_json::json;
use std::{ffi::OsString, path::PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "Cargo with managed intermediate caches and disk-pressure cleanup"
)]
struct Cli {
    /// Override the config file (also configurable through CARGOC_CONFIG)
    #[arg(long = "cargoc-config", global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a user configuration and a dedicated owned cache root
    Init(Settings),
    /// Interactive setup, or one-line enable/disable of the default Cargo shim
    Setup {
        #[command(flatten)]
        settings: Settings,
        /// Enable the user-level Cargo shim without asking
        #[arg(long, conflicts_with_all = ["disable", "status"])]
        enable: bool,
        /// Remove the user-level Cargo shim and restore normal Cargo resolution
        #[arg(long, conflicts_with_all = ["enable", "status"])]
        disable: bool,
        /// Show integration state without changing anything
        #[arg(long, conflicts_with_all = ["enable", "disable"])]
        status: bool,
        /// In setup mode, answer yes to enabling the shim
        #[arg(long)]
        yes: bool,
    },
    /// Display or update user configuration
    Config(Settings),
    /// Show owned intermediate cache usage
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Download and safely install the latest native GitHub Release
    SelfUpdate {
        /// Only check and report whether a newer release exists
        #[arg(long)]
        check: bool,
        /// Restore the newest local rollback backup instead of downloading
        #[arg(long, conflicts_with_all = ["check", "force"])]
        rollback: bool,
        /// Install even when the release version is not newer
        #[arg(long, conflicts_with = "rollback")]
        force: bool,
    },
    /// Clear idle managed intermediate caches, never final Cargo artifacts
    Clean {
        #[arg(long)]
        dry_run: bool,
    },
    /// Forward arguments even when the command name is reserved by cargoc
    Exec {
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Any other command is forwarded to Cargo (e.g. build, check, test, run)
    #[command(external_subcommand)]
    Cargo(Vec<OsString>),
}

#[derive(Args, Default)]
struct Settings {
    #[arg(long)]
    cache_dir: Option<PathBuf>,
    #[arg(long)]
    auto_clean: Option<bool>,
    #[arg(long)]
    max_cache_gib: Option<f64>,
    #[arg(long)]
    low_water_ratio: Option<f64>,
    #[arg(long)]
    min_free_gib: Option<f64>,
    /// Stop only this invocation's process tree below this free space; 0 disables
    #[arg(long)]
    abort_free_gib: Option<f64>,
    /// Expire idle caches after this age; 0 disables age-based collection
    #[arg(long)]
    max_age_days: Option<f64>,
    #[arg(long)]
    poll_seconds: Option<f64>,
    /// Real Cargo executable, not a shell command
    #[arg(long)]
    cargo: Option<PathBuf>,
}

impl Settings {
    fn apply(self, config: &mut Config) -> Result<bool> {
        let mut changed = false;
        if let Some(path) = self.cache_dir {
            config.cache_dir = absolute(&path)?;
            changed = true;
        }
        macro_rules! apply { ($($key:ident),*) => { $(if let Some(value) = self.$key {
            config.$key = value; changed = true;
        })* }; }
        apply!(
            auto_clean,
            max_cache_gib,
            low_water_ratio,
            min_free_gib,
            abort_free_gib,
            max_age_days,
            poll_seconds,
            cargo
        );
        config.validate()?;
        Ok(changed)
    }
}

fn settings(path: &std::path::Path, values: Settings, init: bool) -> Result<()> {
    if init && path.exists() {
        return fail("Configuration exists; use cargoc config to update it");
    }
    let mut config = if init {
        Config::default()
    } else {
        Config::load(path)?
    };
    let changed = values.apply(&mut config)?;
    if init || changed {
        if same_or_descendant(path, &config.cache_dir)? {
            return fail("Store the configuration outside the disposable cache root");
        }
        Store::open(config.clone(), true)?;
        config.save(path)?;
    }
    println!("# {}\n{}", path.display(), toml::to_string_pretty(&config)?);
    Ok(())
}

fn interactive_enable() -> Result<bool> {
    use std::io::{self, IsTerminal, Write};
    if !io::stdin().is_terminal() {
        return fail("Non-interactive setup needs --enable or --disable");
    }
    print!("Enable cargoc as the default cargo command? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn setup(
    path: &std::path::Path,
    values: Settings,
    enable: bool,
    disable: bool,
    status: bool,
    yes: bool,
) -> Result<()> {
    if status {
        return integration::status();
    }
    if disable {
        return integration::disable();
    }
    let mut config = if path.exists() {
        Config::load(path)?
    } else {
        Config::default()
    };
    let explicit_cargo = values.cargo.clone();
    values.apply(&mut config)?;
    if same_or_descendant(path, &config.cache_dir)? {
        return fail("Config must be outside cache_dir");
    }
    let should_enable = if enable || yes {
        true
    } else {
        interactive_enable()?
    };
    if should_enable {
        integration::enable(config, path, explicit_cargo)
    } else {
        Store::open(config.clone(), true)?;
        config.save(path)?;
        println!("cargoc configured at {}", path.display());
        println!(
            "Default Cargo was not replaced. Run `cargoc setup --enable` later to enable the shim."
        );
        Ok(())
    }
}

fn execute(cli: Cli) -> Result<i32> {
    // Updating must work even without a cache configuration, and --check must
    // not trigger legacy config migration.
    if let Commands::SelfUpdate {
        check,
        rollback,
        force,
    } = &cli.command
    {
        self_update::run(*check, *rollback, *force)?;
        return Ok(0);
    }
    let use_default = cli.config.is_none() && std::env::var_os("CARGOC_CONFIG").is_none();
    let path = absolute(&cli.config.unwrap_or_else(default_config_path))?;
    if use_default && migrate_legacy_config(&path, &legacy_config_path())? {
        eprintln!(
            "cargoc: migrated config to {}; legacy config retained",
            path.display()
        );
    }
    match cli.command {
        Commands::Init(values) => settings(&path, values, true)?,
        Commands::Setup {
            settings,
            enable,
            disable,
            status,
            yes,
        } => setup(&path, settings, enable, disable, status, yes)?,
        Commands::Config(values) => settings(&path, values, false)?,
        Commands::Status { json: machine } => {
            let store = Store::open(Config::load(&path)?, false)?;
            let entries = store.entries()?;
            let bytes: u64 = entries.iter().map(|e| e.bytes).sum();
            let free = store.free()?;
            if machine {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({"cache_dir": store.root,
                    "total_bytes": bytes, "free_bytes": free, "config": store.config, "workspaces": entries}))?
                );
            } else {
                println!(
                    "Cache: {}\nManaged size: {:.2} GiB\nDisk free: {:.2} GiB",
                    store.root.display(),
                    bytes as f64 / GIB,
                    free as f64 / GIB
                );
                for entry in entries {
                    println!(
                        "  {:8.2} GiB  {}",
                        entry.bytes as f64 / GIB,
                        entry.record.workspace.display()
                    );
                }
            }
        }
        Commands::SelfUpdate {
            check,
            rollback,
            force,
        } => self_update::run(check, rollback, force)?,
        Commands::Clean { dry_run } => {
            let store = Store::open(Config::load(&path)?, false)?;
            let report = store.collect(&GcOptions {
                all_idle: true,
                dry_run,
                ..GcOptions::default()
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::Exec { args } | Commands::Cargo(args) => {
            runner::install_signal_handler()?;
            return runner::run(Config::load(&path)?, &path, &args);
        }
    }
    Ok(0)
}

fn main() {
    let result = execute(Cli::parse());
    let code = match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("cargoc: {error}");
            2
        }
    };
    std::process::exit(code);
}

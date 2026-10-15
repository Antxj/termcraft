use anyhow::{Context as _, bail};
use base64::Engine as _;
use clap::{Parser, Subcommand};
use serde_json::Value;
use tc_commands::control::SendError;
use tc_commands::{CATALOG, Command, CommandError, Host, execute};
use tc_core::{MacroStore, Paths, SessionStore, Settings, TunnelStore};

#[derive(Parser)]
#[command(name = "termcraft-cli", version, about = "Headless TermCraft")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List every registered command (same set as GUI menus and MCP tools).
    Commands,
    /// Run a registered command: `run session_add '{"session":{...}}'`.
    Run {
        name: String,
        /// JSON object of arguments.
        #[arg(default_value = "{}")]
        args: String,
    },
    /// Send a command to the running TermCraft window (open tabs, type,
    /// run macros…): `gui tab_open '{"session":"web01"}'`.
    Gui {
        name: String,
        #[arg(default_value = "{}")]
        args: String,
    },
    /// List saved sessions.
    Sessions,
    /// Import sessions from a MobaXterm export (.mxtsessions or MobaXterm.ini).
    ImportMobaxterm { path: String },
    /// Serve the Model Context Protocol on stdio for AI agents.
    Mcp,
    /// Self-update and release-signing tools.
    Update {
        #[command(subcommand)]
        cmd: UpdateCmd,
    },
}

#[derive(Subcommand)]
enum UpdateCmd {
    /// Check the configured channel for a newer release.
    Check,
    /// Download, verify and install the newest release.
    Install,
    /// Generate a release-signing keypair (keep the secret in CI secrets).
    Keygen,
    /// Sign a manifest; secret is read from $TERMCRAFT_UPDATE_SECRET.
    Sign { manifest: String },
    /// Print the hex SHA-256 of a file (for manifest entries).
    Hash { file: String },
    /// Write an update manifest for release assets.
    Manifest {
        #[arg(long)]
        version: String,
        #[arg(long, default_value = "stable")]
        channel: String,
        /// URL prefix where the assets are published.
        #[arg(long)]
        base_url: String,
        #[arg(long, default_value = "")]
        notes: String,
        /// Output file.
        #[arg(long)]
        out: String,
        /// `platform=path`, e.g. `windows-x86_64=dist/termcraft-windows-x86_64.exe`.
        assets: Vec<String>,
    },
}

struct FileHost {
    paths: Paths,
    store: SessionStore,
    settings: Settings,
    vault: tc_vault::Vault,
    tunnels: TunnelStore,
    macros: MacroStore,
}

impl FileHost {
    fn open() -> anyhow::Result<Self> {
        let paths = Paths::resolve();
        let store = SessionStore::load(&paths.sessions_file())?;
        let settings = Settings::load(&paths.settings_file())?;
        let vault =
            tc_vault::Vault::open(paths.passwords_index_file(), Box::new(tc_vault::OsKeychain));
        let tunnels = TunnelStore::load(&paths.tunnels_file())?;
        let macros = MacroStore::load(&paths.macros_file())?;
        Ok(Self {
            paths,
            store,
            settings,
            vault,
            tunnels,
            macros,
        })
    }

    fn updater(&self) -> Result<tc_update::Updater, tc_update::UpdateError> {
        tc_update::Updater::from_build(channel_name(&self.settings))
    }
}

impl Host for FileHost {
    fn store(&mut self) -> &mut SessionStore {
        &mut self.store
    }

    fn persist(&mut self) -> Result<(), CommandError> {
        self.store
            .save(&self.paths.sessions_file())
            .map_err(|e| CommandError::Failed(e.to_string()))
    }

    fn vault(&self) -> Result<&tc_vault::Vault, CommandError> {
        Ok(&self.vault)
    }

    fn settings(&mut self) -> Result<&mut Settings, CommandError> {
        Ok(&mut self.settings)
    }

    fn persist_settings(&mut self) -> Result<(), CommandError> {
        self.settings
            .save(&self.paths.settings_file())
            .map_err(|e| CommandError::Failed(e.to_string()))
    }

    fn known_hosts_file(&self) -> Result<std::path::PathBuf, CommandError> {
        Ok(self.paths.known_hosts_file())
    }

    fn macros(&mut self) -> Result<&mut MacroStore, CommandError> {
        Ok(&mut self.macros)
    }

    fn persist_macros(&mut self) -> Result<(), CommandError> {
        self.macros
            .save(&self.paths.macros_file())
            .map_err(|e| CommandError::Failed(e.to_string()))
    }

    fn tunnels(&mut self) -> Result<&mut TunnelStore, CommandError> {
        Ok(&mut self.tunnels)
    }

    fn persist_tunnels(&mut self) -> Result<(), CommandError> {
        self.tunnels
            .save(&self.paths.tunnels_file())
            .map_err(|e| CommandError::Failed(e.to_string()))
    }

    fn update_check(&mut self) -> Result<Value, CommandError> {
        let available = self
            .updater()
            .and_then(|u| u.check(env!("CARGO_PKG_VERSION")))
            .map_err(|e| CommandError::Failed(e.to_string()))?;
        Ok(serde_json::json!({ "current": env!("CARGO_PKG_VERSION"), "available": available }))
    }
}

fn channel_name(settings: &Settings) -> &'static str {
    match settings.update.channel {
        tc_core::settings::UpdateChannel::Stable => "stable",
        tc_core::settings::UpdateChannel::Beta => "beta",
    }
}

/// Run a command where the state lives: in the running window when there
/// is one (it would otherwise overwrite our changes to the files when it
/// saves), else on the files directly.
fn dispatch(host: &mut FileHost, cmd: Command) -> Result<Value, CommandError> {
    let window = match tc_commands::control::request(&host.paths.control_file(), cmd.clone()) {
        Ok(v) => return Ok(v),
        Err(SendError::Failed(e)) => return Err(CommandError::Failed(e)),
        // The window leaves this one (e.g. `update_check`) to us.
        Err(SendError::Unsupported(_)) => true,
        Err(SendError::NotRunning) => false,
    };
    execute(host, cmd).map_err(|e| match e {
        CommandError::Unsupported(why) if !window => {
            CommandError::Failed(format!("{why}: start TermCraft and try again"))
        }
        e => e,
    })
}

fn run_command(host: &mut FileHost, cmd: Command) -> anyhow::Result<()> {
    let out = dispatch(host, cmd)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Commands => {
            for spec in CATALOG {
                let rw = if spec.mutating { "rw" } else { "ro" };
                println!("{:<26} {rw}  {}", spec.name, spec.description);
            }
        }
        Cmd::Run { name, args } => {
            let args: Value = serde_json::from_str(&args).context("args must be JSON")?;
            run_command(&mut FileHost::open()?, Command::from_call(&name, args)?)?;
        }
        Cmd::Gui { name, args } => {
            let args: Value = serde_json::from_str(&args).context("args must be JSON")?;
            let command = Command::from_call(&name, args)?;
            let file = Paths::resolve().control_file();
            match tc_commands::control::send(&file, command) {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v)?),
                Err(e) => bail!(e),
            }
        }
        Cmd::Sessions => run_command(&mut FileHost::open()?, Command::SessionList {})?,
        Cmd::ImportMobaxterm { path } => {
            run_command(
                &mut FileHost::open()?,
                Command::SessionImportMobaxterm { path },
            )?;
        }
        Cmd::Mcp => {
            let mut host = FileHost::open()?;
            let stdin = std::io::stdin().lock();
            tc_mcp::serve_with(
                &mut |cmd| dispatch(&mut host, cmd),
                stdin,
                std::io::stdout().lock(),
            )?;
        }
        Cmd::Update { cmd } => update(cmd)?,
    }
    Ok(())
}

fn update(cmd: UpdateCmd) -> anyhow::Result<()> {
    let b64 = base64::engine::general_purpose::STANDARD;
    match cmd {
        UpdateCmd::Check => run_command(&mut FileHost::open()?, Command::UpdateCheck {})?,
        UpdateCmd::Install => {
            let host = FileHost::open()?;
            let updater = host.updater()?;
            match updater.check(env!("CARGO_PKG_VERSION"))? {
                None => println!("already up to date ({})", env!("CARGO_PKG_VERSION")),
                Some(update) => {
                    println!("installing {} …", update.version);
                    updater.install(&update)?;
                    println!("done; restart TermCraft to use {}", update.version);
                }
            }
        }
        UpdateCmd::Keygen => {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|e| anyhow::anyhow!("rng: {e}"))?;
            let public = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key();
            println!("TERMCRAFT_UPDATE_SECRET={}", b64.encode(seed));
            println!("TERMCRAFT_UPDATE_PUBKEY={}", b64.encode(public.to_bytes()));
        }
        UpdateCmd::Sign { manifest } => {
            let secret =
                std::env::var("TERMCRAFT_UPDATE_SECRET").context("set TERMCRAFT_UPDATE_SECRET")?;
            let bytes = std::fs::read(&manifest)?;
            // Refuse to sign something clients would reject anyway.
            if serde_json::from_slice::<tc_update::Manifest>(&bytes).is_err() {
                bail!("{manifest} is not a valid update manifest");
            }
            let sig = tc_update::sign_manifest(&bytes, &secret)?;
            let out = format!("{manifest}.sig");
            std::fs::write(&out, sig)?;
            println!("wrote {out}");
        }
        UpdateCmd::Hash { file } => {
            println!("{}", tc_update::sha256_file(std::path::Path::new(&file))?);
        }
        UpdateCmd::Manifest {
            version,
            channel,
            base_url,
            notes,
            out,
            assets,
        } => {
            let assets = assets
                .iter()
                .map(|a| {
                    let (platform, path) = a
                        .split_once('=')
                        .with_context(|| format!("asset {a:?} must be platform=path"))?;
                    Ok((platform.to_string(), std::path::PathBuf::from(path)))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let manifest =
                tc_update::build_manifest(&channel, &version, &notes, &base_url, &assets)?;
            std::fs::write(&out, serde_json::to_vec_pretty(&manifest)?)?;
            println!("wrote {out} ({} assets)", manifest.assets.len());
        }
    }
    Ok(())
}

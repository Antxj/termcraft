use anyhow::{Context as _, bail};
use base64::Engine as _;
use clap::{Parser, Subcommand};
use serde_json::Value;
use tc_commands::{CATALOG, Command, CommandError, Host, execute};
use tc_core::{Paths, SessionStore, Settings};

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
}

struct FileHost {
    paths: Paths,
    store: SessionStore,
    settings: Settings,
    vault: tc_vault::Vault,
}

impl FileHost {
    fn open() -> anyhow::Result<Self> {
        let paths = Paths::resolve();
        let store = SessionStore::load(&paths.sessions_file())?;
        let settings = Settings::load(&paths.settings_file())?;
        let vault =
            tc_vault::Vault::open(paths.passwords_index_file(), Box::new(tc_vault::OsKeychain));
        Ok(Self {
            paths,
            store,
            settings,
            vault,
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

fn run_command(host: &mut FileHost, cmd: Command) -> anyhow::Result<()> {
    let out = execute(host, cmd)?;
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
            tc_mcp::serve(&mut host, stdin, std::io::stdout().lock())?;
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
    }
    Ok(())
}

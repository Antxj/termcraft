//! Public-key login through an SSH agent: the system agent (`SSH_AUTH_SOCK`
//! on Unix, the OpenSSH agent pipe on Windows) and PuTTY's Pageant
//! (Windows). Only public keys and signatures cross the agent socket; the
//! private keys never reach TermCraft.

use crate::{Events, Handler, SshEvent};
use russh::client;
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use tokio::io::{AsyncRead, AsyncWrite};

/// Which agents to ask (Configuration › SSH).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentConfig {
    /// "Use Windows SSH agent" / the agent in `SSH_AUTH_SOCK`.
    pub system: bool,
    /// "Use PuTTY agent" (Pageant, Windows only).
    pub pageant: bool,
    /// Agent to use instead of the default one (`SSH_AUTH_SOCK` on Unix,
    /// the OpenSSH pipe on Windows): a socket path or a pipe name.
    pub socket: Option<std::path::PathBuf>,
    /// "Use internal SSH agent": key files offered to every server.
    pub keys: Vec<std::path::PathBuf>,
}

/// Servers drop the connection after `MaxAuthTries` (6 by default), so
/// leave room for the password.
const MAX_AGENT_KEYS: usize = 5;

#[cfg(windows)]
const OPENSSH_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

/// A raw connection to the local agent, for "Allow agent forwarding".
pub(crate) type RawAgent = std::pin::Pin<Box<dyn AsyncReadWrite>>;

pub(crate) trait AsyncReadWrite: AsyncRead + AsyncWrite + Send {}
impl<T: AsyncRead + AsyncWrite + Send> AsyncReadWrite for T {}

/// Open the local agent (the system one, else Pageant on Windows).
pub(crate) async fn connect_raw(config: &AgentConfig) -> Option<RawAgent> {
    if config.system {
        #[cfg(unix)]
        {
            let path = config
                .socket
                .clone()
                .or_else(|| std::env::var_os("SSH_AUTH_SOCK").map(Into::into));
            if let Some(path) = path
                && let Ok(s) = tokio::net::UnixStream::connect(path).await
            {
                return Some(Box::pin(s));
            }
        }
        #[cfg(windows)]
        {
            let pipe = config
                .socket
                .as_ref()
                .map_or_else(|| OPENSSH_PIPE.into(), |p| p.as_os_str().to_owned());
            if let Ok(p) = tokio::net::windows::named_pipe::ClientOptions::new().open(pipe) {
                return Some(Box::pin(p));
            }
        }
    }
    #[cfg(windows)]
    if config.pageant
        && let Ok(p) = pageant::PageantStream::new().await
    {
        return Some(Box::pin(p));
    }
    None
}

/// Try the agents' keys. `Ok(true)` once one is accepted.
pub(crate) async fn authenticate(
    session: &mut client::Handle<Handler>,
    user: &str,
    config: &AgentConfig,
    events: &Events,
) -> Result<bool, String> {
    let mut tried = 0;
    if config.system {
        #[cfg(unix)]
        let agent = match &config.socket {
            Some(path) => AgentClient::connect_uds(path).await,
            None => AgentClient::connect_env().await,
        };
        #[cfg(windows)]
        let agent = match &config.socket {
            Some(pipe) => AgentClient::connect_named_pipe(pipe).await,
            None => AgentClient::connect_named_pipe(OPENSSH_PIPE).await,
        };
        if let Ok(agent) = agent
            && with_agent(session, user, agent, "SSH agent", &mut tried, events).await?
        {
            return Ok(true);
        }
    }
    #[cfg(windows)]
    if config.pageant
        && let Ok(agent) = AgentClient::connect_pageant().await
        && with_agent(session, user, agent, "Pageant", &mut tried, events).await?
    {
        return Ok(true);
    }
    #[cfg(not(windows))]
    let _ = config.pageant;
    Ok(false)
}

async fn with_agent<S>(
    session: &mut client::Handle<Handler>,
    user: &str,
    mut agent: AgentClient<S>,
    name: &str,
    tried: &mut usize,
    events: &Events,
) -> Result<bool, String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Ok(identities) = agent.request_identities().await else {
        return Ok(false);
    };
    let hash = session
        .best_supported_rsa_hash()
        .await
        .map_err(|e| e.to_string())?
        .flatten();
    for identity in identities {
        if *tried >= MAX_AGENT_KEYS {
            break;
        }
        let AgentIdentity::PublicKey { key, comment } = identity else {
            continue; // certificates are not supported yet
        };
        *tried += 1;
        let result = session
            .authenticate_publickey_with(user, key, hash, &mut agent)
            .await
            .map_err(|e| format!("{name}: {e}"))?;
        if result.success() {
            let label = if comment.is_empty() {
                String::new()
            } else {
                format!(" ({comment})")
            };
            events.send(SshEvent::Info(format!(
                "Authenticated with a key from {name}{label}"
            )));
            return Ok(true);
        }
    }
    Ok(false)
}

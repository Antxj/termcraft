//! `cargo run -p tc-devserver [-- --port 2222]`: local SSH server for
//! manual testing (`--features rdp -- --rdp`: the RDP test server). Credentials: docs/DEV.md (override with TC_DEV_USER /
//! TC_DEV_PASSWORD).

use tc_devserver::{DevConfig, bind};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut port = 2222;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                port = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--port needs a value"))?
                    .parse()?
            }
            #[cfg(feature = "rdp")]
            "--rdp" => {
                let server = tc_devserver::rdp::start(tc_devserver::rdp::RdpDevConfig::default())?;
                println!("TermCraft dev RDP server on 127.0.0.1:{}", server.port);
                std::future::pending::<()>().await;
            }
            other => anyhow::bail!("unknown argument {other:?}"),
        }
    }
    let mut config = DevConfig::default();
    if let Ok(user) = std::env::var("TC_DEV_USER") {
        config.user = user;
    }
    if let Ok(password) = std::env::var("TC_DEV_PASSWORD") {
        config.password = password;
    }
    let user = config.user.clone();
    let (addr, server) = bind(port, config).await?;
    println!("TermCraft dev SSH server on {addr} (user {user:?}; password in docs/DEV.md)");
    server.await?;
    Ok(())
}

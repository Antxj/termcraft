//! TermCraft core model. Pure data + persistence; must never depend on UI crates.

pub mod backup;
pub mod hotkeys;
pub mod jump_hosts;
pub mod known_hosts;
pub mod macros;
pub mod monitor;
pub mod paths;
pub mod remote_fs;
pub mod schema;
#[rustfmt::skip]
mod schema_gen;
pub mod session;
pub mod settings;
pub mod store;
pub mod tunnel;

pub use macros::{Macro, MacroId, MacroStore};
pub use paths::Paths;
pub use session::{Session, SessionIcon, SessionId, SessionKind};
pub use settings::Settings;
pub use store::{SessionStore, StoreError};
pub use tunnel::{Tunnel, TunnelId, TunnelKind, TunnelStore};

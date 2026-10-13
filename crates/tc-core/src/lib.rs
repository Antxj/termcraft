//! TermCraft core model. Pure data + persistence; must never depend on UI crates.

pub mod paths;
pub mod session;
pub mod settings;
pub mod store;

pub use paths::Paths;
pub use session::{Session, SessionId, SessionKind};
pub use settings::Settings;
pub use store::{SessionStore, StoreError};

pub mod board;
pub mod client;
pub mod protocol;

/// Display name and channel namespace used by the desktop and project invites.
pub const PROJECTS: &[(&str,&str)] = &[("Neoism","neoism"),("Neoboard","neoboard")];

pub use board::{router, Board, BoardError};

pub fn default_data_dir() -> std::path::PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from(".")).join("neochat")
}
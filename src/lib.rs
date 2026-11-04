//! A Cargo wrapper that owns and bounds intermediate build caches.
//! Final Cargo artifacts are never garbage-collected by this library.
pub mod audit;
pub mod cargo;
pub mod config;
pub mod installation;
pub mod integration;
pub mod native_shim;
mod process;
pub mod runner;
pub mod self_update;
pub mod store;
pub mod updates;
pub mod util;
#[cfg(windows)]
mod windows_path;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

pub fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(std::io::Error::other(message.into()).into())
}

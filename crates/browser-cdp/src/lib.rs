//! Direct Chrome DevTools Protocol implementation of the browser abstraction.

mod connection;
mod discovery;
mod host;
mod page;
mod process;
mod session;
mod wire;

pub use discovery::find_system_chrome;
pub use host::ChromeHost;

#[cfg(test)]
mod tests;

//! Protocol-independent browser semantics used by the test runtime.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Locator {
    Id(String),
}

impl std::fmt::Display for Locator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Id(value) => write!(f, "id({value:?})"),
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum BrowserError {
    #[error("no element matching {locator} was found")]
    LocatorNotFound { locator: Locator },
    #[error("locator {locator} matched {matches} elements")]
    LocatorAmbiguous { locator: Locator, matches: usize },
    #[error("navigation to {url} failed: {reason}")]
    NavigationFailed { url: String, reason: String },
    #[error("the browser disconnected")]
    BrowserDisconnected,
    #[error("CDP method {method} failed: {message}")]
    Protocol { method: String, message: String },
    #[error("could not launch the browser: {0}")]
    Launch(String),
}

#[async_trait]
pub trait BrowserHost: Send + Sync {
    async fn start(&self) -> Result<Box<dyn BrowserSession>, BrowserError>;
}

#[async_trait]
pub trait BrowserSession: Send {
    async fn new_page(&mut self) -> Result<Box<dyn Page>, BrowserError>;
}

#[async_trait]
pub trait Page: Send {
    async fn open(&mut self, url: &str) -> Result<(), BrowserError>;
    async fn click(&mut self, locator: &Locator) -> Result<(), BrowserError>;
}

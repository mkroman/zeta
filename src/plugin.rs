use std::cell::OnceCell;
use std::sync::{Mutex, OnceLock};

use tracing::{debug, trace};

pub struct GoogleSearch;

impl Plugin for GoogleSearch {
    fn new() -> GoogleSearch {
        GoogleSearch {}
    }

    fn name() -> &'static str {
        "google_search"
    }
}

pub trait Plugin: Send + Sync {
    fn name() -> &'static str
    where
        Self: Sized;

    fn new() -> Self
    where
        Self: Sized;
}

#[derive(Default)]
pub struct Registry {
    plugins: Vec<Box<dyn Plugin>>,
}

impl Registry {
    /// Constructs and returns a new, empty plugin registry.
    pub fn new() -> Registry {
        Registry { plugins: vec![] }
    }

    /// Constructs and returns a new plugin registry with initialized plugins.
    pub fn loaded() -> Registry {
        let mut registry = Self::new();
        trace!("Registering plugins");
        registry.register::<GoogleSearch>();

        let num_plugins = registry.plugins.len();
        trace!(%num_plugins, "Done registering plugins");
        registry
    }

    /// Registers a new plugin based on its type.
    pub fn register<P: Plugin + 'static>(&mut self) -> bool {
        let plugin = Box::new(P::new());

        self.plugins.push(plugin);

        true
    }
}

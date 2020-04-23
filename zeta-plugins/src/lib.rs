use std::sync::Mutex;

use lazy_static::lazy_static;

mod google_search;
mod plugin;

use plugin::Plugin;

lazy_static! {}

pub struct PluginRegistry {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginRegistry {
    /// Constructs a new plugin registry
    pub fn new() -> PluginRegistry {
        PluginRegistry { plugins: vec![] }
    }

    /// Registers a new plugin
    pub fn register<P: Plugin + 'static>(&mut self) -> bool {
        let plugin = Box::new(P::new());

        self.plugins.push(plugin);

        true
    }
}

lazy_static! {
    static ref PLUGIN_REGISTRY: Mutex<PluginRegistry> = Mutex::new(PluginRegistry::new());
}

pub fn init() {
    let mut registry = PLUGIN_REGISTRY.lock().unwrap();

    registry.register::<google_search::GoogleSearch>();
}

mod skills;
mod combat;
mod camera;
mod commands;

use pumpkin::plugin::Plugin;
use pumpkin::plugin::PluginFuture;
use std::sync::{Arc, LazyLock};

pub static GLOBAL_RUNTIME: LazyLock<std::sync::Arc<tokio::runtime::Runtime>> =
    LazyLock::new(|| std::sync::Arc::new(tokio::runtime::Runtime::new().unwrap()));

pub struct RpgPlugin;

impl RpgPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Plugin for RpgPlugin {
    fn on_load(&self, server: Arc<pumpkin::plugin::Context>) -> PluginFuture<'_, Result<(), String>> {
        Box::pin(async move {
            server.log("Pumpkin RPG Plugin loading...");
            commands::register_all(&server).await?;
            server.log("Pumpkin RPG Plugin loaded! Use /skill list, /camera list, /rpgclass info");
            Ok(())
        })
    }

    fn on_unload(
        &self,
        server: Arc<pumpkin::plugin::Context>,
    ) -> PluginFuture<'_, Result<(), String>> {
        Box::pin(async move {
            server.log("Pumpkin RPG Plugin unloaded.");
            Ok(())
        })
    }
}

#[unsafe(no_mangle)]
pub static METADATA: LazyLock<pumpkin::plugin::PluginMetadata> = LazyLock::new(|| {
    pumpkin::plugin::PluginMetadata {
        name: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        authors: env!("CARGO_PKG_AUTHORS")
            .split(',')
            .map(String::from)
            .collect(),
        description: env!("CARGO_PKG_DESCRIPTION").to_string(),
        dependencies: Vec::new(),
        permissions: Vec::new(),
    }
});

#[unsafe(no_mangle)]
pub static PUMPKIN_API_VERSION: u32 = pumpkin::plugin::PLUGIN_API_VERSION;

#[unsafe(no_mangle)]
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(RpgPlugin::new())
}

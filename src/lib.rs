#![forbid(unsafe_code)]

mod api;
mod checks;
mod commands;
mod config;
mod events;
mod lang;
mod model;
mod prefs;
mod runtime;
mod safe;

use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use strawberry_plugin_api::{
    Context, Plugin, PluginMetadata, Result,
    permission::{Permission, PermissionDefault},
    permissions, register_plugin, wit,
};

struct TpaLite {
    runtime: Mutex<Option<runtime::Shared>>,
}

impl Plugin for TpaLite {
    fn new() -> Self {
        Self {
            runtime: Mutex::new(None),
        }
    }
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "tpa-lite".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            authors: vec!["Strawberry Interactive LLC".into()],
            description: lang::text("en", "description", &[]),
            dependencies: vec![],
            permissions: vec![
                permissions::FS_READ_DATA.into(),
                permissions::FS_WRITE_DATA.into(),
            ],
        }
    }
    fn on_load(&self, context: Context) -> Result<()> {
        let data = context.get_data_folder();
        let data = Path::new(&data);
        let refuse = |key: &'static str| lang::text("en", key, &[]);
        let config = config::Config::load(&data.join("tpa.properties")).map_err(refuse)?;
        let messages = lang::Overrides::load(&data.join("lang")).map_err(refuse)?;
        let preferences = runtime::read_preferences(data).map_err(refuse)?;
        let shared = Arc::new(runtime::Runtime::new(
            config,
            messages,
            preferences,
            data.to_path_buf(),
        ));
        for (node, key) in [
            (runtime::USE, "desc_tpa"),
            (runtime::HERE, "desc_here"),
            (runtime::ACCEPT, "desc_accept"),
            (runtime::DENY, "desc_deny"),
            (runtime::CANCEL, "desc_cancel"),
            (runtime::TOGGLE, "desc_toggle"),
        ] {
            context.register_permission(&Permission {
                node: node.into(),
                description: lang::text("en", key, &[]),
                default: PermissionDefault::Allow,
                children: vec![],
            })?;
        }
        commands::register(&context, &shared);
        events::register(&context, &shared)?;
        *runtime::lock(&self.runtime) = Some(shared);
        Ok(())
    }
    fn on_unload(&self, _context: Context) -> Result<()> {
        let shared = { runtime::lock(&self.runtime).take() };
        if let Some(shared) = shared {
            shared.stop();
        }
        Ok(())
    }
    fn handle_ipc_message(
        &self,
        from: wit::PluginId,
        payload: wit::IpcMessage,
    ) -> std::result::Result<wit::IpcMessage, String> {
        let shared = { runtime::lock(&self.runtime).clone() };
        let shared = shared.ok_or_else(|| "tpa-lite is not loaded".to_owned())?;
        runtime::ipc(&shared, &from, &payload)
    }
}
register_plugin!(TpaLite);

//! Context-menu actions registered by plugins.
//!
//! A registration is only useful if the file manager remembers which plugin
//! made it: choosing the action has to run *that* plugin. The registry keys
//! every action by its owner, and the manager drops a plugin's actions when it
//! unloads so the menu never offers something nobody will answer.

use std::sync::{Arc, RwLock};

use raven_core::events::PluginActionInfo;

/// An action one plugin registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAction {
    pub plugin_id: String,
    /// What the plugin is told was chosen.
    pub name: String,
    /// What the menu shows.
    pub label: String,
}

impl From<&PluginAction> for PluginActionInfo {
    fn from(action: &PluginAction) -> Self {
        PluginActionInfo {
            plugin_id: action.plugin_id.clone(),
            name: action.name.clone(),
            label: action.label.clone(),
        }
    }
}

/// Every plugin's registered actions, shared by the manager and the per-plugin
/// API handles that register into it.
#[derive(Debug, Clone, Default)]
pub struct ActionRegistry {
    actions: Arc<RwLock<Vec<PluginAction>>>,
}

impl ActionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `name` for `plugin_id`. Registering a name the plugin already
    /// has updates its label in place, so a plugin that registers on every
    /// load does not fill the menu with copies.
    pub fn register(&self, plugin_id: &str, name: &str, label: &str) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("an action needs a name".to_string());
        }
        let label = if label.trim().is_empty() { name } else { label };
        let mut actions = self.actions.write().map_err(|e| format!("Lock error: {}", e))?;
        match actions
            .iter_mut()
            .find(|a| a.plugin_id == plugin_id && a.name == name)
        {
            Some(existing) => existing.label = label.to_string(),
            None => actions.push(PluginAction {
                plugin_id: plugin_id.to_string(),
                name: name.to_string(),
                label: label.to_string(),
            }),
        }
        Ok(())
    }

    /// Forget everything `plugin_id` registered.
    pub fn remove_plugin(&self, plugin_id: &str) {
        if let Ok(mut actions) = self.actions.write() {
            actions.retain(|a| a.plugin_id != plugin_id);
        }
    }

    /// Whether `plugin_id` registered `name`.
    pub fn contains(&self, plugin_id: &str, name: &str) -> bool {
        self.actions
            .read()
            .map(|actions| {
                actions
                    .iter()
                    .any(|a| a.plugin_id == plugin_id && a.name == name)
            })
            .unwrap_or(false)
    }

    /// All actions, grouped by plugin in registration order within each.
    /// The grouping keeps the menu stable when plugins load in a different
    /// order from one run to the next.
    pub fn list(&self) -> Vec<PluginAction> {
        let mut actions = self
            .actions
            .read()
            .map(|a| a.clone())
            .unwrap_or_default();
        // Stable sort: order inside one plugin is the order it registered in.
        actions.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
        actions
    }

    /// The list in the form sent to the window.
    pub fn snapshot(&self) -> Vec<PluginActionInfo> {
        self.list().iter().map(PluginActionInfo::from).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_list_groups_by_plugin() {
        let reg = ActionRegistry::new();
        reg.register("zeta", "b", "B").unwrap();
        reg.register("alpha", "x", "X").unwrap();
        reg.register("zeta", "a", "A").unwrap();

        let names: Vec<(String, String)> = reg
            .list()
            .into_iter()
            .map(|a| (a.plugin_id, a.name))
            .collect();
        assert_eq!(
            names,
            vec![
                ("alpha".to_string(), "x".to_string()),
                ("zeta".to_string(), "b".to_string()),
                ("zeta".to_string(), "a".to_string()),
            ]
        );
    }

    #[test]
    fn re_registering_updates_the_label() {
        let reg = ActionRegistry::new();
        reg.register("p", "zip", "Compress").unwrap();
        reg.register("p", "zip", "Compress to .zip").unwrap();
        let list = reg.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].label, "Compress to .zip");
    }

    #[test]
    fn same_name_from_two_plugins_is_two_actions() {
        let reg = ActionRegistry::new();
        reg.register("a", "run", "Run A").unwrap();
        reg.register("b", "run", "Run B").unwrap();
        assert_eq!(reg.list().len(), 2);
        assert!(reg.contains("a", "run"));
        assert!(reg.contains("b", "run"));
        assert!(!reg.contains("c", "run"));
    }

    #[test]
    fn empty_name_is_rejected_and_empty_label_falls_back() {
        let reg = ActionRegistry::new();
        assert!(reg.register("p", "  ", "Label").is_err());
        reg.register("p", "go", "").unwrap();
        assert_eq!(reg.list()[0].label, "go");
    }

    #[test]
    fn remove_plugin_drops_only_its_actions() {
        let reg = ActionRegistry::new();
        reg.register("a", "one", "One").unwrap();
        reg.register("b", "two", "Two").unwrap();
        reg.remove_plugin("a");
        let list = reg.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].plugin_id, "b");
    }

    #[test]
    fn snapshot_carries_owner_name_and_label() {
        let reg = ActionRegistry::new();
        reg.register("p", "n", "L").unwrap();
        let snap = reg.snapshot();
        assert_eq!(
            snap,
            vec![PluginActionInfo {
                plugin_id: "p".into(),
                name: "n".into(),
                label: "L".into(),
            }]
        );
    }
}

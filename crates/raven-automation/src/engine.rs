use std::collections::HashMap;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use raven_core::automation_types::{AutomationRule, Trigger};
use raven_core::events::AppEvent;

use crate::action::execute_action;
use crate::condition::evaluate_conditions;
use crate::scheduler::parse_cron_interval;

/// The automation engine that ties rules, filesystem watchers, and actions together.
pub struct AutomationEngine {
    rules: Vec<AutomationRule>,
    event_tx: mpsc::UnboundedSender<AppEvent>,
    watcher_handles: HashMap<String, tokio::task::JoinHandle<()>>,
    running: bool,
}

impl AutomationEngine {
    /// Create a new `AutomationEngine` that emits events on the provided channel.
    pub fn new(event_tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self {
            rules: Vec::new(),
            event_tx,
            watcher_handles: HashMap::new(),
            running: false,
        }
    }

    /// Add a rule to the engine.
    pub fn add_rule(&mut self, rule: AutomationRule) {
        info!(rule_id = %rule.id, name = %rule.name, "adding automation rule");
        self.rules.push(rule);
    }

    /// Remove a rule by ID. Returns `true` if the rule was found and removed.
    pub fn remove_rule(&mut self, rule_id: &str) -> bool {
        let len_before = self.rules.len();

        // Cancel the watcher for this rule if running
        if let Some(handle) = self.watcher_handles.remove(rule_id) {
            handle.abort();
        }

        self.rules.retain(|r| r.id != rule_id);
        let removed = self.rules.len() < len_before;
        if removed {
            info!(rule_id, "removed automation rule");
        }
        removed
    }

    /// Enable or disable a rule. Returns `true` if the rule was found.
    pub fn set_rule_enabled(&mut self, rule_id: &str, enabled: bool) -> bool {
        if let Some(rule) = self.rules.iter_mut().find(|r| r.id == rule_id) {
            rule.enabled = enabled;
            info!(rule_id, enabled, "set automation rule enabled state");
            true
        } else {
            false
        }
    }

    /// Start the engine -- sets up filesystem watchers for all enabled rules.
    pub async fn start(&mut self) {
        if self.running {
            warn!("automation engine is already running");
            return;
        }

        info!("starting automation engine with {} rules", self.rules.len());
        self.running = true;

        // Snapshot the rules to avoid borrow issues
        let rules: Vec<AutomationRule> = self.rules.clone();

        for rule in &rules {
            if !rule.enabled {
                continue;
            }

            match &rule.trigger {
                Trigger::FileCreated | Trigger::FileModified | Trigger::FileDeleted => {
                    self.start_watcher(rule);
                }
                Trigger::Schedule { cron } => {
                    self.start_scheduler(rule, cron);
                }
                Trigger::Manual => {
                    // Manual rules are only triggered via trigger_rule()
                    info!(rule_id = %rule.id, "manual rule registered, awaiting trigger");
                }
            }
        }
    }

    /// Stop the engine -- cancels all watchers and schedulers.
    pub async fn stop(&mut self) {
        if !self.running {
            return;
        }

        info!("stopping automation engine");

        for (rule_id, handle) in self.watcher_handles.drain() {
            handle.abort();
            info!(rule_id, "stopped watcher");
        }

        self.running = false;
    }

    /// Manually trigger a specific rule on its watch paths.
    pub async fn trigger_rule(&self, rule_id: &str) {
        let rule = match self.rules.iter().find(|r| r.id == rule_id) {
            Some(r) => r.clone(),
            None => {
                warn!(rule_id, "cannot trigger: rule not found");
                return;
            }
        };

        info!(rule_id, "manually triggering rule");
        process_rule_on_paths(&rule, &self.event_tx).await;
    }

    /// Get a reference to all rules.
    pub fn rules(&self) -> &[AutomationRule] {
        &self.rules
    }

    /// Start a filesystem watcher for a rule with a file-event trigger.
    fn start_watcher(&mut self, rule: &AutomationRule) {
        let rule = rule.clone();
        let event_tx = self.event_tx.clone();
        let rule_id_for_map = rule.id.clone();

        let handle = tokio::task::spawn(async move {
            let rule_id = rule.id.clone();
            if let Err(e) = run_watcher(rule, event_tx).await {
                error!(rule_id = %rule_id, error = %e, "watcher task failed");
            }
        });

        self.watcher_handles.insert(rule_id_for_map, handle);
    }

    /// Start a scheduled task for a rule with a cron trigger.
    fn start_scheduler(&mut self, rule: &AutomationRule, cron: &str) {
        let interval = match parse_cron_interval(cron) {
            Some(d) => d,
            None => {
                warn!(
                    rule_id = %rule.id,
                    cron,
                    "unsupported cron expression, skipping scheduler"
                );
                let _ = self.event_tx.send(AppEvent::AutomationError {
                    rule_id: rule.id.clone(),
                    error: format!("unsupported cron expression: {}", cron),
                });
                return;
            }
        };

        let rule = rule.clone();
        let event_tx = self.event_tx.clone();
        let rule_id_for_map = rule.id.clone();

        let handle = tokio::task::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            // Skip the first tick which fires immediately
            timer.tick().await;

            loop {
                timer.tick().await;
                info!(rule_id = %rule.id, "scheduled rule triggered");
                process_rule_on_paths(&rule, &event_tx).await;
            }
        });

        self.watcher_handles.insert(rule_id_for_map, handle);
    }
}

/// Run a filesystem watcher for a single rule, blocking until the task is cancelled.
async fn run_watcher(
    rule: AutomationRule,
    event_tx: mpsc::UnboundedSender<AppEvent>,
) -> Result<(), String> {
    // Use a tokio mpsc channel to bridge notify's synchronous callback into async.
    let (bridge_tx, mut bridge_rx) = mpsc::unbounded_channel::<notify::Event>();

    let trigger = rule.trigger.clone();
    let _watcher = {
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = bridge_tx.send(event);
                }
            },
            notify::Config::default(),
        )
        .map_err(|e| format!("failed to create watcher: {}", e))?;

        for path in &rule.watch_paths {
            if path.exists() {
                watcher
                    .watch(path, RecursiveMode::NonRecursive)
                    .map_err(|e| {
                        format!("failed to watch path {}: {}", path.display(), e)
                    })?;
                info!(rule_id = %rule.id, path = %path.display(), "watching directory");
            } else {
                warn!(
                    rule_id = %rule.id,
                    path = %path.display(),
                    "watch path does not exist, skipping"
                );
            }
        }

        watcher
    };

    // Process notify events from the bridge channel.
    while let Some(event) = bridge_rx.recv().await {
        // Filter events by trigger type
        let should_process = match (&trigger, &event.kind) {
            (Trigger::FileCreated, EventKind::Create(_)) => true,
            (Trigger::FileModified, EventKind::Modify(_)) => true,
            (Trigger::FileDeleted, EventKind::Remove(_)) => true,
            _ => false,
        };

        if !should_process {
            continue;
        }

        for event_path in &event.paths {
            // For non-delete triggers, only process files (not directories).
            if !matches!(trigger, Trigger::FileDeleted) && !event_path.is_file() {
                continue;
            }

            let matches = evaluate_conditions(
                &rule.conditions,
                &rule.condition_mode,
                event_path,
            );

            if !matches {
                continue;
            }

            let file_str = event_path.to_string_lossy().to_string();

            // Emit rule triggered event
            let _ = event_tx.send(AppEvent::AutomationRuleTriggered {
                rule_id: rule.id.clone(),
                rule_name: rule.name.clone(),
                matched_files: vec![file_str],
            });

            // Execute actions
            for action in &rule.actions {
                match execute_action(action, event_path).await {
                    Ok(desc) => {
                        let _ = event_tx.send(AppEvent::AutomationActionCompleted {
                            rule_id: rule.id.clone(),
                            action: desc,
                        });
                    }
                    Err(e) => {
                        error!(
                            rule_id = %rule.id,
                            path = %event_path.display(),
                            error = %e,
                            "action execution failed"
                        );
                        let _ = event_tx.send(AppEvent::AutomationError {
                            rule_id: rule.id.clone(),
                            error: e,
                        });
                    }
                }
            }
        }
    }

    Ok(())
}

/// Scan watch paths for a rule and process all matching files.
async fn process_rule_on_paths(
    rule: &AutomationRule,
    event_tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let mut all_matched = Vec::new();

    for watch_path in &rule.watch_paths {
        let mut entries = match tokio::fs::read_dir(watch_path).await {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    rule_id = %rule.id,
                    path = %watch_path.display(),
                    error = %e,
                    "failed to read watch path"
                );
                let _ = event_tx.send(AppEvent::AutomationError {
                    rule_id: rule.id.clone(),
                    error: format!(
                        "failed to read {}: {}",
                        watch_path.display(),
                        e
                    ),
                });
                continue;
            }
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            if evaluate_conditions(&rule.conditions, &rule.condition_mode, &path) {
                all_matched.push(path);
            }
        }
    }

    if all_matched.is_empty() {
        return;
    }

    let matched_strs: Vec<String> = all_matched
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let _ = event_tx.send(AppEvent::AutomationRuleTriggered {
        rule_id: rule.id.clone(),
        rule_name: rule.name.clone(),
        matched_files: matched_strs,
    });

    for path in &all_matched {
        for action in &rule.actions {
            match execute_action(action, path).await {
                Ok(desc) => {
                    let _ = event_tx.send(AppEvent::AutomationActionCompleted {
                        rule_id: rule.id.clone(),
                        action: desc,
                    });
                }
                Err(e) => {
                    error!(
                        rule_id = %rule.id,
                        path = %path.display(),
                        error = %e,
                        "action execution failed"
                    );
                    let _ = event_tx.send(AppEvent::AutomationError {
                        rule_id: rule.id.clone(),
                        error: e,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use raven_core::automation_types::*;
    use tempfile::TempDir;

    fn make_rule(id: &str, trigger: Trigger, watch_paths: Vec<PathBuf>) -> AutomationRule {
        AutomationRule {
            id: id.to_string(),
            name: format!("Rule {}", id),
            enabled: true,
            trigger,
            conditions: vec![],
            condition_mode: ConditionMode::All,
            actions: vec![],
            watch_paths,
        }
    }

    #[tokio::test]
    async fn test_engine_add_and_remove_rules() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut engine = AutomationEngine::new(tx);

        engine.add_rule(make_rule("r1", Trigger::Manual, vec![]));
        engine.add_rule(make_rule("r2", Trigger::Manual, vec![]));
        assert_eq!(engine.rules().len(), 2);

        assert!(engine.remove_rule("r1"));
        assert_eq!(engine.rules().len(), 1);
        assert_eq!(engine.rules()[0].id, "r2");

        assert!(!engine.remove_rule("nonexistent"));
    }

    #[tokio::test]
    async fn test_engine_set_rule_enabled() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut engine = AutomationEngine::new(tx);

        engine.add_rule(make_rule("r1", Trigger::Manual, vec![]));
        assert!(engine.rules()[0].enabled);

        assert!(engine.set_rule_enabled("r1", false));
        assert!(!engine.rules()[0].enabled);

        assert!(engine.set_rule_enabled("r1", true));
        assert!(engine.rules()[0].enabled);

        assert!(!engine.set_rule_enabled("nonexistent", false));
    }

    #[tokio::test]
    async fn test_engine_start_stop() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut engine = AutomationEngine::new(tx);

        engine.add_rule(make_rule("r1", Trigger::Manual, vec![]));

        engine.start().await;
        assert!(engine.running);

        engine.stop().await;
        assert!(!engine.running);
    }

    #[tokio::test]
    async fn test_trigger_manual_rule() {
        let dir = TempDir::new().unwrap();

        // Create a test file
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, b"hello").await.unwrap();

        let dest_dir = TempDir::new().unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut engine = AutomationEngine::new(tx);

        let rule = AutomationRule {
            id: "manual-move".to_string(),
            name: "Manual Move".to_string(),
            enabled: true,
            trigger: Trigger::Manual,
            conditions: vec![Condition::ExtensionIs {
                extensions: vec!["txt".to_string()],
            }],
            condition_mode: ConditionMode::All,
            actions: vec![Action::CopyTo {
                destination: dest_dir.path().to_path_buf(),
            }],
            watch_paths: vec![dir.path().to_path_buf()],
        };

        engine.add_rule(rule);
        engine.trigger_rule("manual-move").await;

        // Should receive rule triggered event
        let event = rx.recv().await.unwrap();
        match event {
            AppEvent::AutomationRuleTriggered {
                rule_id,
                rule_name,
                matched_files,
            } => {
                assert_eq!(rule_id, "manual-move");
                assert_eq!(rule_name, "Manual Move");
                assert_eq!(matched_files.len(), 1);
            }
            other => panic!("expected AutomationRuleTriggered, got: {:?}", other),
        }

        // Should receive action completed event
        let event = rx.recv().await.unwrap();
        match event {
            AppEvent::AutomationActionCompleted { rule_id, action } => {
                assert_eq!(rule_id, "manual-move");
                assert!(action.contains("Copied"));
            }
            other => panic!("expected AutomationActionCompleted, got: {:?}", other),
        }

        // Verify the file was copied
        assert!(dest_dir.path().join("test.txt").exists());
    }

    #[tokio::test]
    async fn test_trigger_nonexistent_rule() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let engine = AutomationEngine::new(tx);

        // Should not panic
        engine.trigger_rule("nonexistent").await;
    }

    #[tokio::test]
    async fn test_trigger_rule_no_matching_files() {
        let dir = TempDir::new().unwrap();

        // Create a .jpg file but rule only matches .pdf
        let file_path = dir.path().join("photo.jpg");
        tokio::fs::write(&file_path, b"image data").await.unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut engine = AutomationEngine::new(tx);

        let rule = AutomationRule {
            id: "no-match".to_string(),
            name: "No Match Rule".to_string(),
            enabled: true,
            trigger: Trigger::Manual,
            conditions: vec![Condition::ExtensionIs {
                extensions: vec!["pdf".to_string()],
            }],
            condition_mode: ConditionMode::All,
            actions: vec![Action::Notify {
                message: "matched".to_string(),
            }],
            watch_paths: vec![dir.path().to_path_buf()],
        };

        engine.add_rule(rule);
        engine.trigger_rule("no-match").await;

        // No events should be sent since nothing matched
        // Give a small window then check
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_trigger_rule_with_action_error() {
        let dir = TempDir::new().unwrap();

        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, b"data").await.unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut engine = AutomationEngine::new(tx);

        let rule = AutomationRule {
            id: "error-rule".to_string(),
            name: "Error Rule".to_string(),
            enabled: true,
            trigger: Trigger::Manual,
            conditions: vec![],
            condition_mode: ConditionMode::All,
            actions: vec![Action::RunCommand {
                command: "nonexistent_command_xyz".to_string(),
                args: vec![],
            }],
            watch_paths: vec![dir.path().to_path_buf()],
        };

        engine.add_rule(rule);
        engine.trigger_rule("error-rule").await;

        // Should receive rule triggered event
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, AppEvent::AutomationRuleTriggered { .. }));

        // Should receive error event
        let event = rx.recv().await.unwrap();
        match event {
            AppEvent::AutomationError { rule_id, error } => {
                assert_eq!(rule_id, "error-rule");
                assert!(error.contains("nonexistent_command_xyz"));
            }
            other => panic!("expected AutomationError, got: {:?}", other),
        }
    }
}

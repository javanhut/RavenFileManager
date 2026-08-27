use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use raven_ai::duplicates::{DuplicateScanConfig, DuplicateScanner};
use raven_ai::nl_search;
use raven_ai::organize::{OrganizationAnalyzer, OrganizeConfig};
use raven_ai::tag_engine::TagEngine;
use raven_automation::config as automation_config;
use raven_automation::engine::AutomationEngine;
use raven_core::commands::AppCommand;
use raven_core::commands::SearchMode;
use raven_core::config::AppConfig;
use raven_core::dir_size_cache::DirSizeCache;
use raven_core::entry::FileEntry;
use raven_core::events::AppEvent;
use raven_core::operations::{Operation, OperationId, OperationKind};
use raven_core::path::RavenPath;
use raven_core::sort::SortSpec;
use raven_core::vfs::VirtualFileSystem;
use raven_dbus::service::DbusService;
use raven_ops::executor::{OperationExecutor, ProgressReporter};
use raven_ops::queue::OperationQueue;
use raven_ops::undo::UndoStack;
use raven_plugin::api::MockPluginApi;
use raven_plugin::manager::PluginManager;
use raven_preview::router::PreviewRouter;
use raven_search::content::{ContentSearchConfig, ContentSearcher};
use raven_search::recursive::{RecursiveSearchConfig, RecursiveSearcher};
use raven_system::container::ContainerInspector;
use raven_system::disk_usage::DiskUsageCalculator;
use raven_system::package_lookup::PackageLookup;
use raven_system::process_lock::ProcessLockDetector;
use raven_system::systemd::SystemdInspector;
use raven_ui::app::RavenApplication;
use raven_vfs::router::VfsRouter;

/// Cache entries older than this are revalidated in the background.
const CACHE_TTL: Duration = Duration::from_secs(30);

/// Debounce window for filesystem watcher events.
const WATCHER_DEBOUNCE: Duration = Duration::from_millis(200);

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("Starting Raven File Manager");

    let config = AppConfig::load();

    let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<AppCommand>();
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();
    let command_tx_for_dbus = command_tx.clone();

    // Spawn the Tokio runtime on a separate thread
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime");

        rt.block_on(async move {
            // --- Core systems ---
            let vfs_router = Arc::new(VfsRouter::new());
            let vfs: Arc<dyn VirtualFileSystem> = vfs_router.clone();
            let queue = Arc::new(OperationQueue::new());
            let undo_stack = Arc::new(UndoStack::new());
            let executor = Arc::new(
                OperationExecutor::new(queue.clone(), undo_stack.clone())
                    .expect("Failed to create operation executor"),
            );
            let preview_router = Arc::new(PreviewRouter::new());
            let next_op_id = Arc::new(std::sync::atomic::AtomicU64::new(1));

            // --- Directory size cache ---
            let dir_size_cache = Arc::new(DirSizeCache::new());
            let current_pane_id = Arc::new(std::sync::atomic::AtomicU32::new(0));

            // --- Filesystem watcher ---
            let (fs_event_tx, mut fs_event_rx) =
                tokio::sync::mpsc::unbounded_channel::<notify::Event>();
            let watched_path: Arc<std::sync::Mutex<Option<PathBuf>>> =
                Arc::new(std::sync::Mutex::new(None));
            let fs_watcher: Arc<std::sync::Mutex<Option<notify::RecommendedWatcher>>> = {
                let tx = fs_event_tx;
                match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                    if let Ok(event) = res {
                        let _ = tx.send(event);
                    }
                }) {
                    Ok(w) => Arc::new(std::sync::Mutex::new(Some(w))),
                    Err(e) => {
                        tracing::warn!("Failed to create filesystem watcher: {}", e);
                        Arc::new(std::sync::Mutex::new(None))
                    }
                }
            };

            // Spawn watcher debounce task
            {
                let cache = dir_size_cache.clone();
                let event_tx = event_tx.clone();
                let pane_id_ref = current_pane_id.clone();
                let watched_path = watched_path.clone();
                tokio::spawn(async move {
                    loop {
                        // Wait for the first event
                        let first = match fs_event_rx.recv().await {
                            Some(ev) => ev,
                            None => break,
                        };

                        // Collect affected paths
                        let mut affected_dirs: HashSet<PathBuf> = HashSet::new();
                        for p in &first.paths {
                            if let Some(parent) = p.parent() {
                                affected_dirs.insert(parent.to_path_buf());
                            }
                        }

                        // Debounce: collect events for WATCHER_DEBOUNCE duration
                        let deadline =
                            tokio::time::Instant::now() + WATCHER_DEBOUNCE;
                        loop {
                            match tokio::time::timeout_at(deadline, fs_event_rx.recv()).await {
                                Ok(Some(ev)) => {
                                    for p in &ev.paths {
                                        if let Some(parent) = p.parent() {
                                            affected_dirs.insert(parent.to_path_buf());
                                        }
                                    }
                                }
                                _ => break,
                            }
                        }

                        // Process: invalidate and recalculate affected directories
                        let pane_id =
                            pane_id_ref.load(std::sync::atomic::Ordering::Relaxed);
                        let current_watched = watched_path.lock().unwrap().clone();

                        for dir_path in affected_dirs {
                            // Only process if the affected dir is the watched dir
                            // or a direct child of it (a subdirectory we display)
                            let is_relevant = current_watched.as_ref().map_or(false, |w| {
                                dir_path == *w || dir_path.parent() == Some(w.as_path())
                            });
                            if !is_relevant {
                                continue;
                            }

                            cache.invalidate(&dir_path);
                            let size = calculate_dir_size(&dir_path).await;
                            cache.set(&dir_path, size);
                            let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                pane_id,
                                path: RavenPath::Local(dir_path),
                                size,
                            });
                        }
                    }
                });
            }

            // --- Automation engine ---
            let mut automation_engine = AutomationEngine::new(event_tx.clone());
            let rules_dir = config
                .automation
                .rules_dir
                .as_ref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(automation_config::default_rules_dir);
            let rules = automation_config::load_rules(&rules_dir);
            for rule in rules {
                automation_engine.add_rule(rule);
            }
            if config.automation.enabled {
                automation_engine.start().await;
                tracing::info!("Automation engine started");
            }

            // --- Plugin manager ---
            let mut plugin_manager = PluginManager::new(event_tx.clone());
            for dir in &config.plugins.plugin_dirs {
                plugin_manager.add_plugin_dir(std::path::PathBuf::from(dir));
            }
            // Also add default plugin dirs
            for dir in PluginManager::default_plugin_dirs() {
                plugin_manager.add_plugin_dir(dir);
            }
            // Auto-load enabled plugins
            let discovered = plugin_manager.discover();
            let plugin_api = Arc::new(MockPluginApi::new());
            for (dir, manifest) in &discovered {
                if config.plugins.enabled_plugins.contains(&manifest.id)
                    || config.plugins.enabled_plugins.is_empty()
                {
                    match plugin_manager.load_plugin(dir, plugin_api.clone()) {
                        Ok(id) => tracing::info!("Loaded plugin: {}", id),
                        Err(e) => tracing::warn!("Failed to load plugin from {:?}: {}", dir, e),
                    }
                }
            }

            // --- DBus service ---
            let dbus_service = Arc::new(DbusService::new(
                command_tx_for_dbus,
                config.dbus.bus_name.clone(),
            ));
            if config.dbus.enabled {
                tracing::info!(
                    "DBus service initialized with bus name: {}",
                    dbus_service.bus_name()
                );
            }

            // --- System integration ---
            let package_lookup = Arc::new(PackageLookup::new());
            let disk_usage_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));

            // --- AI features ---
            let tags_dir = config
                .automation
                .rules_dir
                .as_ref()
                .map(|d| std::path::PathBuf::from(d).parent().unwrap_or(std::path::Path::new(".")).to_path_buf())
                .unwrap_or_else(|| {
                    std::env::var_os("XDG_CONFIG_HOME")
                        .map(std::path::PathBuf::from)
                        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join("raven")
                });
            let tag_engine = std::sync::Arc::new(tokio::sync::Mutex::new(
                TagEngine::new(tags_dir.join("tags.toml")),
            ));
            let duplicate_scan_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));

            tracing::info!("Backend runtime started");

            while let Some(command) = command_rx.recv().await {
                let event_tx = event_tx.clone();
                let vfs = vfs.clone();
                let vfs_router = vfs_router.clone();
                let executor = executor.clone();
                let queue = queue.clone();
                let undo_stack = undo_stack.clone();
                let preview_router = preview_router.clone();
                let next_op_id = next_op_id.clone();
                let dbus_service = dbus_service.clone();
                let package_lookup = package_lookup.clone();
                let disk_usage_cancel = disk_usage_cancel.clone();
                let tag_engine = tag_engine.clone();
                let duplicate_scan_cancel = duplicate_scan_cancel.clone();
                let dir_size_cache = dir_size_cache.clone();
                let current_pane_id = current_pane_id.clone();
                let fs_watcher = fs_watcher.clone();
                let watched_path = watched_path.clone();

                match command {
                    // --- Navigation (cache-first) ---
                    AppCommand::Navigate { path, pane_id } => {
                        // Track the active pane for operation-triggered cache updates
                        current_pane_id.store(pane_id, std::sync::atomic::Ordering::Relaxed);

                        // Update filesystem watcher to the new directory
                        if let Some(local) = path.as_local_path() {
                            let new_path = local.clone();
                            let mut wp = watched_path.lock().unwrap();
                            if let Ok(mut watcher_guard) = fs_watcher.lock() {
                                if let Some(ref mut watcher) = *watcher_guard {
                                    // Unwatch old path
                                    if let Some(ref old) = *wp {
                                        let _ = watcher.unwatch(old);
                                    }
                                    // Watch new path
                                    if let Err(e) = watcher.watch(&new_path, RecursiveMode::NonRecursive) {
                                        tracing::debug!("Watcher failed for {}: {}", new_path.display(), e);
                                    }
                                }
                            }
                            *wp = Some(new_path);
                        }

                        let cache = dir_size_cache.clone();
                        tokio::spawn(async move {
                            tracing::info!("Navigating to: {}", path);
                            match vfs.list_dir(&path).await {
                                Ok(mut entries) => {
                                    sort_entries(&mut entries, &SortSpec::default());

                                    // Collect directory paths for size calculation
                                    let dir_paths: Vec<RavenPath> = entries
                                        .iter()
                                        .filter(|e| e.is_dir())
                                        .map(|e| e.path.clone())
                                        .collect();

                                    // Tag counts come from the entries we already listed,
                                    // so navigation never re-reads the directory for them.
                                    let tag_counts = {
                                        let engine = tag_engine.lock().await;
                                        engine.tag_counts(&entries)
                                    };

                                    let event = AppEvent::DirectoryLoaded {
                                        pane_id,
                                        path,
                                        entries,
                                    };
                                    dbus_service.handle_event(&event).await;
                                    let _ = event_tx.send(event);

                                    let _ = event_tx.send(AppEvent::TagCountsUpdated {
                                        pane_id,
                                        counts: tag_counts,
                                    });

                                    // Phase 1 + 2: cache-first with background validation
                                    for dir_path in dir_paths {
                                        if let Some(local) = dir_path.as_local_path().cloned() {
                                            let cache = cache.clone();
                                            let event_tx = event_tx.clone();

                                            // Phase 1: serve cached value instantly
                                            let cached = cache.get(&local);
                                            if let Some(ref entry) = cached {
                                                let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                                    pane_id,
                                                    path: dir_path.clone(),
                                                    size: entry.size,
                                                });
                                            }

                                            // Phase 2: background validation for uncached or stale entries
                                            let needs_revalidation = match &cached {
                                                None => true,
                                                Some(entry) => entry.computed_at.elapsed() > CACHE_TTL,
                                            };

                                            if needs_revalidation {
                                                tokio::spawn(async move {
                                                    let generation = cache.invalidate(&local);
                                                    let size = calculate_dir_size(&local).await;

                                                    if cache.insert_if_current(&local, size, generation) {
                                                        // Only send UI update if size differs from cached
                                                        let should_notify = cached
                                                            .map(|e| e.size != size)
                                                            .unwrap_or(true);
                                                        if should_notify {
                                                            let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                                                pane_id,
                                                                path: dir_path,
                                                                size,
                                                            });
                                                        }
                                                    }
                                                });
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::DirectoryError {
                                        pane_id,
                                        path,
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    // --- File Operations (with cache invalidation) ---
                    AppCommand::CopyFiles {
                        sources,
                        destination,
                    } => {
                        let op_id = OperationId(
                            next_op_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                        );
                        let op = Operation::new(
                            op_id,
                            OperationKind::Copy,
                            sources.clone(),
                            Some(destination.clone()),
                        );
                        let desc = format!(
                            "Copying {} item(s) to {}",
                            sources.len(),
                            destination
                        );
                        let _ = event_tx.send(AppEvent::OperationStarted {
                            id: op_id,
                            description: desc,
                        });

                        let cache = dir_size_cache.clone();
                        let pane_id = current_pane_id.load(std::sync::atomic::Ordering::Relaxed);
                        tokio::spawn(async move {
                            let reporter = make_progress_reporter(op_id, event_tx.clone());
                            match executor.execute(&op, vfs.as_ref(), Some(reporter)).await {
                                Ok(()) => {
                                    // Invalidate destination directory and recalculate
                                    if let Some(dest_local) = destination.as_local_path() {
                                        cache.invalidate(dest_local);
                                        let size = calculate_dir_size(dest_local).await;
                                        cache.set(dest_local, size);
                                        let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                            pane_id,
                                            path: destination,
                                            size,
                                        });
                                    }
                                    let event = AppEvent::OperationCompleted { id: op_id };
                                    dbus_service.handle_event(&event).await;
                                    let _ = event_tx.send(event);
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::OperationFailed {
                                        id: op_id,
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::MoveFiles {
                        sources,
                        destination,
                    } => {
                        let op_id = OperationId(
                            next_op_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                        );
                        let op = Operation::new(
                            op_id,
                            OperationKind::Move,
                            sources.clone(),
                            Some(destination.clone()),
                        );
                        let desc = format!(
                            "Moving {} item(s) to {}",
                            sources.len(),
                            destination
                        );
                        let _ = event_tx.send(AppEvent::OperationStarted {
                            id: op_id,
                            description: desc,
                        });

                        let cache = dir_size_cache.clone();
                        let pane_id = current_pane_id.load(std::sync::atomic::Ordering::Relaxed);
                        tokio::spawn(async move {
                            // Capture source sizes before the move for delta calculation
                            let mut source_size_total: i64 = 0;
                            let mut source_parents: HashSet<PathBuf> = HashSet::new();
                            for src in &sources {
                                if let Some(local) = src.as_local_path() {
                                    if let Ok(meta) = tokio::fs::metadata(local).await {
                                        if meta.is_dir() {
                                            let size = cache.get(local)
                                                .map(|e| e.size)
                                                .unwrap_or_else(|| {
                                                    // Will be recalculated; use 0 as fallback
                                                    0
                                                });
                                            source_size_total += size as i64;
                                        } else {
                                            source_size_total += meta.len() as i64;
                                        }
                                    }
                                    if let Some(parent) = local.parent() {
                                        source_parents.insert(parent.to_path_buf());
                                    }
                                }
                            }

                            let reporter = make_progress_reporter(op_id, event_tx.clone());
                            match executor.execute(&op, vfs.as_ref(), Some(reporter)).await {
                                Ok(()) => {
                                    // Invalidate source parents
                                    for parent in &source_parents {
                                        if source_size_total > 0 {
                                            if let Some(new_size) = cache.apply_delta(parent, -source_size_total) {
                                                let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                                    pane_id,
                                                    path: RavenPath::Local(parent.clone()),
                                                    size: new_size,
                                                });
                                            } else {
                                                // Not cached, invalidate and recalculate
                                                cache.invalidate(parent);
                                                let size = calculate_dir_size(parent).await;
                                                cache.set(parent, size);
                                                let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                                    pane_id,
                                                    path: RavenPath::Local(parent.clone()),
                                                    size,
                                                });
                                            }
                                        }
                                    }

                                    // Evict moved directories from cache
                                    for src in &sources {
                                        if let Some(local) = src.as_local_path() {
                                            cache.evict_tree(local);
                                        }
                                    }

                                    // Invalidate destination and recalculate
                                    if let Some(dest_local) = destination.as_local_path() {
                                        cache.invalidate(dest_local);
                                        let size = calculate_dir_size(dest_local).await;
                                        cache.set(dest_local, size);
                                        let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                            pane_id,
                                            path: destination,
                                            size,
                                        });
                                    }

                                    let event = AppEvent::OperationCompleted { id: op_id };
                                    dbus_service.handle_event(&event).await;
                                    let _ = event_tx.send(event);
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::OperationFailed {
                                        id: op_id,
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::DeleteFiles { paths } => {
                        let op_id = OperationId(
                            next_op_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                        );
                        let op = Operation::new(op_id, OperationKind::Delete, paths.clone(), None);
                        let desc = format!("Deleting {} item(s)", paths.len());
                        let _ = event_tx.send(AppEvent::OperationStarted {
                            id: op_id,
                            description: desc,
                        });

                        let cache = dir_size_cache.clone();
                        let pane_id = current_pane_id.load(std::sync::atomic::Ordering::Relaxed);
                        tokio::spawn(async move {
                            // Stat files before deletion to capture sizes for delta updates
                            let mut file_infos: Vec<(PathBuf, u64, bool)> = Vec::new();
                            for p in &paths {
                                if let Some(local) = p.as_local_path() {
                                    if let Ok(meta) = tokio::fs::metadata(local).await {
                                        if meta.is_dir() {
                                            let size = cache.get(local)
                                                .map(|e| e.size)
                                                .unwrap_or_else(|| {
                                                    // Can't async-calculate synchronously here,
                                                    // we'll invalidate parent instead
                                                    0
                                                });
                                            file_infos.push((local.clone(), size, true));
                                        } else {
                                            file_infos.push((local.clone(), meta.len(), false));
                                        }
                                    }
                                }
                            }

                            match executor.execute(&op, vfs.as_ref(), None).await {
                                Ok(()) => {
                                    // Apply cache deltas
                                    let mut invalidated_parents: HashSet<PathBuf> = HashSet::new();
                                    for (local_path, size, is_dir) in &file_infos {
                                        if *is_dir {
                                            cache.evict_tree(local_path);
                                        }
                                        if let Some(parent) = local_path.parent() {
                                            if *size > 0 {
                                                let delta = -(*size as i64);
                                                if let Some(new_size) = cache.apply_delta(parent, delta) {
                                                    let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                                        pane_id,
                                                        path: RavenPath::Local(parent.to_path_buf()),
                                                        size: new_size,
                                                    });
                                                    // Propagate to ancestors
                                                    for (ancestor, a_size) in cache.propagate_delta_to_ancestors(parent, delta) {
                                                        let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                                            pane_id,
                                                            path: RavenPath::Local(ancestor),
                                                            size: a_size,
                                                        });
                                                    }
                                                } else {
                                                    invalidated_parents.insert(parent.to_path_buf());
                                                }
                                            } else {
                                                invalidated_parents.insert(parent.to_path_buf());
                                            }
                                        }
                                    }

                                    // For parents we couldn't apply deltas to, recalculate
                                    for parent in invalidated_parents {
                                        cache.invalidate(&parent);
                                        let size = calculate_dir_size(&parent).await;
                                        cache.set(&parent, size);
                                        let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                            pane_id,
                                            path: RavenPath::Local(parent),
                                            size,
                                        });
                                    }

                                    let _ = event_tx.send(AppEvent::OperationCompleted { id: op_id });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::OperationFailed {
                                        id: op_id,
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::TrashFiles { paths } => {
                        let op_id = OperationId(
                            next_op_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                        );
                        let op = Operation::new(op_id, OperationKind::Trash, paths.clone(), None);
                        let desc = format!("Trashing {} item(s)", paths.len());
                        let _ = event_tx.send(AppEvent::OperationStarted {
                            id: op_id,
                            description: desc,
                        });

                        let cache = dir_size_cache.clone();
                        let pane_id = current_pane_id.load(std::sync::atomic::Ordering::Relaxed);
                        tokio::spawn(async move {
                            // Stat files before trashing to capture sizes for delta updates
                            let mut file_infos: Vec<(PathBuf, u64, bool)> = Vec::new();
                            for p in &paths {
                                if let Some(local) = p.as_local_path() {
                                    if let Ok(meta) = tokio::fs::metadata(local).await {
                                        if meta.is_dir() {
                                            let size = cache.get(local)
                                                .map(|e| e.size)
                                                .unwrap_or(0);
                                            file_infos.push((local.clone(), size, true));
                                        } else {
                                            file_infos.push((local.clone(), meta.len(), false));
                                        }
                                    }
                                }
                            }

                            match executor.execute(&op, vfs.as_ref(), None).await {
                                Ok(()) => {
                                    let mut invalidated_parents: HashSet<PathBuf> = HashSet::new();
                                    for (local_path, size, is_dir) in &file_infos {
                                        if *is_dir {
                                            cache.evict_tree(local_path);
                                        }
                                        if let Some(parent) = local_path.parent() {
                                            if *size > 0 {
                                                let delta = -(*size as i64);
                                                if let Some(new_size) = cache.apply_delta(parent, delta) {
                                                    let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                                        pane_id,
                                                        path: RavenPath::Local(parent.to_path_buf()),
                                                        size: new_size,
                                                    });
                                                    for (ancestor, a_size) in cache.propagate_delta_to_ancestors(parent, delta) {
                                                        let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                                            pane_id,
                                                            path: RavenPath::Local(ancestor),
                                                            size: a_size,
                                                        });
                                                    }
                                                } else {
                                                    invalidated_parents.insert(parent.to_path_buf());
                                                }
                                            } else {
                                                invalidated_parents.insert(parent.to_path_buf());
                                            }
                                        }
                                    }

                                    for parent in invalidated_parents {
                                        cache.invalidate(&parent);
                                        let size = calculate_dir_size(&parent).await;
                                        cache.set(&parent, size);
                                        let _ = event_tx.send(AppEvent::DirSizeCalculated {
                                            pane_id,
                                            path: RavenPath::Local(parent),
                                            size,
                                        });
                                    }

                                    let _ = event_tx.send(AppEvent::OperationCompleted { id: op_id });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::OperationFailed {
                                        id: op_id,
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::RenameFile { path, new_name } => {
                        let op_id = OperationId(
                            next_op_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                        );
                        let dest = if let Some(parent) = path.parent() {
                            parent.join(&new_name)
                        } else {
                            continue;
                        };
                        let op = Operation::new(
                            op_id,
                            OperationKind::Rename,
                            vec![path.clone()],
                            Some(dest.clone()),
                        );

                        let cache = dir_size_cache.clone();
                        tokio::spawn(async move {
                            match executor.execute(&op, vfs.as_ref(), None).await {
                                Ok(()) => {
                                    // Update cache key: remove old, insert new with same size
                                    if let Some(old_local) = path.as_local_path() {
                                        if let Some(entry) = cache.get(old_local) {
                                            if let Some(new_local) = dest.as_local_path() {
                                                cache.set(new_local, entry.size);
                                            }
                                            cache.evict_tree(old_local);
                                        }
                                    }
                                    let _ = event_tx.send(AppEvent::OperationCompleted { id: op_id });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::OperationFailed {
                                        id: op_id,
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::CreateDirectory { parent, name } => {
                        let dest = parent.join(&name);
                        let op_id = OperationId(
                            next_op_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                        );
                        let op = Operation::new(
                            op_id,
                            OperationKind::CreateDirectory,
                            vec![],
                            Some(dest),
                        );

                        tokio::spawn(async move {
                            match executor.execute(&op, vfs.as_ref(), None).await {
                                Ok(()) => {
                                    let _ = event_tx.send(AppEvent::OperationCompleted { id: op_id });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::OperationFailed {
                                        id: op_id,
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::CreateFile { parent, name } => {
                        let dest = parent.join(&name);
                        let op_id = OperationId(
                            next_op_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                        );
                        let op = Operation::new(
                            op_id,
                            OperationKind::CreateFile,
                            vec![],
                            Some(dest),
                        );

                        tokio::spawn(async move {
                            match executor.execute(&op, vfs.as_ref(), None).await {
                                Ok(()) => {
                                    let _ = event_tx.send(AppEvent::OperationCompleted { id: op_id });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::OperationFailed {
                                        id: op_id,
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    // --- Operation control ---
                    AppCommand::PauseOperation { id } => {
                        let _ = queue.pause(id).await;
                    }

                    AppCommand::ResumeOperation { id } => {
                        let _ = queue.resume(id).await;
                    }

                    AppCommand::CancelOperation { id } => {
                        let _ = queue.cancel(id).await;
                    }

                    AppCommand::Undo => {
                        let undo_stack = undo_stack.clone();
                        let vfs = vfs.clone();
                        let executor = executor.clone();
                        tokio::spawn(async move {
                            match undo_stack.undo(vfs.as_ref(), executor.trash_fs()).await {
                                Ok(record) => {
                                    tracing::info!("Undo: {:?}", record.kind);
                                    let _ = event_tx.send(AppEvent::OperationCompleted {
                                        id: record.operation_id,
                                    });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::Notification {
                                        title: "Undo failed".to_string(),
                                        message: e.to_string(),
                                        level: raven_core::events::NotificationLevel::Error,
                                    });
                                }
                            }
                        });
                    }

                    // --- Search ---
                    AppCommand::Search {
                        query,
                        path,
                        search_mode,
                    } => {
                        tokio::spawn(async move {
                            match search_mode {
                                SearchMode::Filename | SearchMode::Regex => {
                                    let searcher = RecursiveSearcher::new();
                                    let local_path = path.as_local_path().cloned().unwrap_or_default();
                                    let config = RecursiveSearchConfig {
                                        root: local_path,
                                        query,
                                        use_regex: search_mode == SearchMode::Regex,
                                        ..RecursiveSearchConfig::default()
                                    };
                                    match searcher.search(config) {
                                        Ok(mut rx) => {
                                            let mut count = 0u64;
                                            while let Some(m) = rx.recv().await {
                                                count += 1;
                                                let _ = event_tx.send(AppEvent::SearchResult {
                                                    path: m.entry.path.clone(),
                                                    entry: m.entry,
                                                });
                                            }
                                            let _ = event_tx.send(AppEvent::SearchCompleted {
                                                total_matches: count,
                                            });
                                        }
                                        Err(e) => {
                                            let _ = event_tx.send(AppEvent::SearchError {
                                                error: e.to_string(),
                                            });
                                        }
                                    }
                                }
                                SearchMode::NaturalLanguage => {
                                    let parsed = nl_search::parse_nl_query(&query);
                                    let searcher = RecursiveSearcher::new();
                                    let local_path = path.as_local_path().cloned().unwrap_or_default();
                                    let search_query = parsed.filename_pattern.clone();
                                    let config = RecursiveSearchConfig {
                                        root: local_path,
                                        query: search_query,
                                        use_regex: false,
                                        ..RecursiveSearchConfig::default()
                                    };
                                    match searcher.search(config) {
                                        Ok(mut rx) => {
                                            let mut count = 0u64;
                                            while let Some(m) = rx.recv().await {
                                                // Post-filter by NL criteria
                                                let entry = &m.entry;

                                                // Filter by file types
                                                if !parsed.file_types.is_empty() {
                                                    let matches_type = parsed.file_types.iter().any(|ft| {
                                                        match ft {
                                                            raven_core::filter::FileTypeFilter::Images => {
                                                                matches!(entry.extension(), Some("jpg" | "jpeg" | "png" | "gif" | "bmp" | "svg" | "webp" | "tiff" | "raw" | "ico"))
                                                            }
                                                            raven_core::filter::FileTypeFilter::Videos => {
                                                                matches!(entry.extension(), Some("mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm"))
                                                            }
                                                            raven_core::filter::FileTypeFilter::Audio => {
                                                                matches!(entry.extension(), Some("mp3" | "flac" | "ogg" | "wav" | "aac" | "wma" | "m4a" | "opus"))
                                                            }
                                                            raven_core::filter::FileTypeFilter::Documents => {
                                                                matches!(entry.extension(), Some("pdf" | "doc" | "docx" | "odt" | "txt" | "rtf" | "md" | "tex" | "epub"))
                                                            }
                                                            raven_core::filter::FileTypeFilter::Archives => {
                                                                matches!(entry.extension(), Some("zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "zst"))
                                                            }
                                                            raven_core::filter::FileTypeFilter::Directories => entry.is_dir(),
                                                            raven_core::filter::FileTypeFilter::Files => entry.is_file(),
                                                            raven_core::filter::FileTypeFilter::Symlinks => entry.kind == raven_core::entry::EntryKind::Symlink,
                                                            raven_core::filter::FileTypeFilter::Custom(_) => true,
                                                        }
                                                    });
                                                    if !matches_type {
                                                        continue;
                                                    }
                                                }

                                                // Filter by size
                                                if let Some(min) = parsed.min_size {
                                                    if entry.metadata.size < min {
                                                        continue;
                                                    }
                                                }
                                                if let Some(max) = parsed.max_size {
                                                    if entry.metadata.size > max {
                                                        continue;
                                                    }
                                                }

                                                // Filter by modified date
                                                if let Some(after) = parsed.modified_after {
                                                    if let Some(modified) = entry.metadata.modified {
                                                        if modified < after {
                                                            continue;
                                                        }
                                                    } else {
                                                        continue;
                                                    }
                                                }
                                                if let Some(before) = parsed.modified_before {
                                                    if let Some(modified) = entry.metadata.modified {
                                                        if modified > before {
                                                            continue;
                                                        }
                                                    } else {
                                                        continue;
                                                    }
                                                }

                                                // Filter by extensions
                                                if !parsed.extensions.is_empty() {
                                                    if let Some(ext) = entry.extension() {
                                                        if !parsed.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                                                            continue;
                                                        }
                                                    } else {
                                                        continue;
                                                    }
                                                }

                                                count += 1;
                                                let _ = event_tx.send(AppEvent::SearchResult {
                                                    path: m.entry.path.clone(),
                                                    entry: m.entry,
                                                });
                                            }
                                            let _ = event_tx.send(AppEvent::SearchCompleted {
                                                total_matches: count,
                                            });
                                        }
                                        Err(e) => {
                                            let _ = event_tx.send(AppEvent::SearchError {
                                                error: e.to_string(),
                                            });
                                        }
                                    }
                                }
                                SearchMode::Content => {
                                    let searcher = ContentSearcher::new();
                                    let local_path = path.as_local_path().cloned().unwrap_or_default();
                                    let config = ContentSearchConfig::new(local_path, &query);
                                    match searcher.search(config) {
                                        Ok(mut rx) => {
                                            let mut count = 0u64;
                                            while let Some(m) = rx.recv().await {
                                                count += 1;
                                                let entry = raven_core::entry::FileEntry::new(
                                                    format!(
                                                        "{}:{} {}",
                                                        m.path.display(),
                                                        m.line_number,
                                                        m.line
                                                    ),
                                                    raven_core::path::RavenPath::Local(m.path.clone()),
                                                    raven_core::entry::EntryKind::File,
                                                    raven_core::entry::EntryMetadata::default(),
                                                );
                                                let _ = event_tx.send(AppEvent::SearchResult {
                                                    path: raven_core::path::RavenPath::Local(m.path),
                                                    entry,
                                                });
                                            }
                                            let _ = event_tx.send(AppEvent::SearchCompleted {
                                                total_matches: count,
                                            });
                                        }
                                        Err(e) => {
                                            let _ = event_tx.send(AppEvent::SearchError {
                                                error: e.to_string(),
                                            });
                                        }
                                    }
                                }
                            }
                        });
                    }

                    AppCommand::CancelSearch => {
                        tracing::debug!("Search cancellation requested");
                    }

                    // --- Filter ---
                    AppCommand::SetFilter { filter, pane_id } => {
                        let _ = event_tx.send(AppEvent::FilterApplied { filter, pane_id });
                    }

                    // --- Sort ---
                    AppCommand::SetSort { sort, pane_id: _ } => {
                        tracing::debug!("Sort changed to {:?}", sort);
                    }

                    // --- Preview ---
                    AppCommand::GeneratePreview { path } => {
                        tokio::spawn(async move {
                            match preview_router.preview(&path).await {
                                Ok(preview) => {
                                    let _ = event_tx.send(AppEvent::PreviewReady {
                                        path,
                                        preview,
                                    });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::PreviewError {
                                        path,
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::CancelPreview => {
                        tracing::debug!("Preview cancellation requested");
                    }

                    // --- Git ---
                    AppCommand::RefreshGitStatus { path } => {
                        tokio::spawn(async move {
                            if let Some(local_path) = path.as_local_path() {
                                let statuses =
                                    raven_git::status::GitStatusProvider::get_status_entries(
                                        local_path,
                                    );
                                if !statuses.is_empty() {
                                    let _ = event_tx.send(AppEvent::GitStatusUpdated {
                                        path,
                                        statuses,
                                    });
                                }
                            }
                        });
                    }

                    // --- Refresh ---
                    AppCommand::Refresh { pane_id: _ } => {
                        tracing::debug!("Refresh not directly handled in backend");
                    }

                    // --- Navigation handled in UI ---
                    AppCommand::NavigateBack { .. }
                    | AppCommand::NavigateForward { .. }
                    | AppCommand::NavigateUp { .. } => {}

                    AppCommand::ResolveConflict { id, strategy } => {
                        tracing::debug!("Conflict resolution: {:?} for {:?}", strategy, id);
                    }

                    // --- Automation ---
                    AppCommand::StartAutomation => {
                        automation_engine.start().await;
                        tracing::info!("Automation engine started");
                    }

                    AppCommand::StopAutomation => {
                        automation_engine.stop().await;
                        tracing::info!("Automation engine stopped");
                    }

                    AppCommand::AddAutomationRule { rule } => {
                        let rule_id = rule.id.clone();
                        automation_engine.add_rule(rule);
                        tracing::info!("Added automation rule: {}", rule_id);
                    }

                    AppCommand::RemoveAutomationRule { rule_id } => {
                        if automation_engine.remove_rule(&rule_id) {
                            tracing::info!("Removed automation rule: {}", rule_id);
                        } else {
                            tracing::warn!("Automation rule not found: {}", rule_id);
                        }
                    }

                    AppCommand::EnableAutomationRule { rule_id } => {
                        automation_engine.set_rule_enabled(&rule_id, true);
                    }

                    AppCommand::DisableAutomationRule { rule_id } => {
                        automation_engine.set_rule_enabled(&rule_id, false);
                    }

                    AppCommand::TriggerAutomationRule { rule_id } => {
                        automation_engine.trigger_rule(&rule_id).await;
                    }

                    // --- Plugins ---
                    AppCommand::LoadPlugin { path } => {
                        let api = Arc::new(MockPluginApi::new());
                        match plugin_manager.load_plugin(&path, api) {
                            Ok(id) => tracing::info!("Loaded plugin: {}", id),
                            Err(e) => {
                                let _ = event_tx.send(AppEvent::PluginError {
                                    plugin_id: path.display().to_string(),
                                    error: e,
                                });
                            }
                        }
                    }

                    AppCommand::UnloadPlugin { plugin_id } => {
                        match plugin_manager.unload_plugin(&plugin_id) {
                            Ok(()) => tracing::info!("Unloaded plugin: {}", plugin_id),
                            Err(e) => {
                                let _ = event_tx.send(AppEvent::PluginError {
                                    plugin_id,
                                    error: e,
                                });
                            }
                        }
                    }

                    // --- Network connections ---
                    AppCommand::ConnectSftp {
                        host,
                        port,
                        user,
                        auth,
                    } => {
                        let host_clone = host.clone();
                        tokio::spawn(async move {
                            match vfs_router.connect_sftp(host.clone(), port, user.clone(), &auth).await {
                                Ok(key) => {
                                    let _ = event_tx.send(AppEvent::RemoteConnected {
                                        id: key,
                                        protocol: "sftp".to_string(),
                                        host: host_clone,
                                    });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::RemoteError {
                                        id: format!("{}@{}:{}", user, host_clone, port),
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::ConnectSmb {
                        host,
                        share,
                        user: _,
                        password: _,
                    } => {
                        let _ = event_tx.send(AppEvent::RemoteError {
                            id: format!("smb://{}/{}", host, share),
                            error: "SMB support not yet available".to_string(),
                        });
                    }

                    AppCommand::DisconnectRemote { id } => {
                        let vfs_router = vfs_router.clone();
                        let id_clone = id.clone();
                        tokio::spawn(async move {
                            match vfs_router.disconnect_sftp(&id_clone).await {
                                Ok(()) => {
                                    let _ = event_tx.send(AppEvent::RemoteDisconnected {
                                        id: id_clone,
                                    });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::RemoteError {
                                        id: id_clone,
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    // --- System integration ---
                    AppCommand::GetPackageOwner { path } => {
                        tokio::spawn(async move {
                            match package_lookup.query_owner(&path).await {
                                Ok(package) => {
                                    let _ = event_tx.send(AppEvent::PackageOwnerResult {
                                        path,
                                        package,
                                    });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::SystemError {
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::GetProcessLocks { path } => {
                        tokio::spawn(async move {
                            match ProcessLockDetector::check_locks(&path).await {
                                Ok(locks) => {
                                    let _ = event_tx.send(AppEvent::ProcessLocksResult {
                                        path,
                                        locks,
                                    });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::SystemError {
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::CalculateDiskUsage { path } => {
                        disk_usage_cancel.store(false, std::sync::atomic::Ordering::SeqCst);
                        let cancel = disk_usage_cancel.clone();
                        tokio::spawn(async move {
                            let local_path = match path.as_local_path() {
                                Some(p) => p.clone(),
                                None => {
                                    let _ = event_tx.send(AppEvent::DiskUsageError {
                                        path,
                                        error: "Disk usage only supported for local paths".into(),
                                    });
                                    return;
                                }
                            };
                            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                            let path_for_task = path.clone();
                            let event_tx_progress = event_tx.clone();

                            // Forward streaming entries as progress events
                            let forward_handle = tokio::spawn(async move {
                                while let Some(entry) = rx.recv().await {
                                    let _ = event_tx_progress.send(AppEvent::DiskUsageProgress {
                                        path: path_for_task.clone(),
                                        entry,
                                    });
                                }
                            });

                            match DiskUsageCalculator::calculate(&local_path, tx, cancel).await {
                                Ok((total_size, total_items)) => {
                                    let _ = forward_handle.await;
                                    let _ = event_tx.send(AppEvent::DiskUsageCompleted {
                                        path,
                                        total_size,
                                        total_items,
                                    });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::DiskUsageError {
                                        path,
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::CancelDiskUsage => {
                        disk_usage_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                    }

                    AppCommand::GetContainerInfo => {
                        tokio::spawn(async move {
                            let info = ContainerInspector::detect().await;
                            let _ = event_tx.send(AppEvent::ContainerInfoResult { info });
                        });
                    }

                    AppCommand::InspectSystemdUnit { path } => {
                        tokio::spawn(async move {
                            match SystemdInspector::parse_unit_file(&path).await {
                                Ok(mut unit) => {
                                    // Also try to get the active state
                                    if let Ok(Some(status)) =
                                        SystemdInspector::get_unit_status(&unit.name).await
                                    {
                                        unit.active_state = Some(status);
                                    }
                                    let _ = event_tx.send(AppEvent::SystemdUnitResult {
                                        path,
                                        unit,
                                    });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::SystemError {
                                        error: e.to_string(),
                                    });
                                }
                            }
                        });
                    }

                    // --- AI features ---
                    AppCommand::ScanDuplicates {
                        path,
                        recursive,
                        min_size,
                    } => {
                        duplicate_scan_cancel.store(false, std::sync::atomic::Ordering::SeqCst);
                        let cancel = duplicate_scan_cancel.clone();
                        tokio::spawn(async move {
                            let local_path = match path.as_local_path() {
                                Some(p) => p.clone(),
                                None => {
                                    let _ = event_tx.send(AppEvent::DuplicateScanError {
                                        error: "Duplicate scan only supported for local paths".into(),
                                    });
                                    return;
                                }
                            };
                            let scanner = DuplicateScanner::new(cancel);
                            let config = DuplicateScanConfig {
                                root: local_path,
                                recursive,
                                min_size,
                                include_hidden: false,
                            };
                            let (progress_tx, mut progress_rx) =
                                tokio::sync::mpsc::unbounded_channel();

                            let event_tx_progress = event_tx.clone();
                            let forward_handle = tokio::spawn(async move {
                                while let Some(progress) = progress_rx.recv().await {
                                    let _ = event_tx_progress.send(
                                        AppEvent::DuplicateScanProgress { progress },
                                    );
                                }
                            });

                            match scanner.scan(config, progress_tx).await {
                                Ok(groups) => {
                                    let _ = forward_handle.await;
                                    let _ = event_tx.send(AppEvent::DuplicateScanCompleted { groups });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::DuplicateScanError { error: e });
                                }
                            }
                        });
                    }

                    AppCommand::CancelDuplicateScan => {
                        duplicate_scan_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                    }

                    AppCommand::RefreshTagCounts { pane_id, path } => {
                        let tag_engine = tag_engine.clone();
                        let vfs = vfs.clone();
                        tokio::spawn(async move {
                            emit_tag_counts(&vfs, &tag_engine, &event_tx, &path, pane_id).await;
                        });
                    }

                    AppCommand::AddManualTag { path, tag } => {
                        // Applied inline so a RefreshTagCounts queued right behind this
                        // command observes the new tag.
                        let mut engine = tag_engine.lock().await;
                        engine.add_manual_tag(&path.to_string_lossy(), &tag);
                        if let Err(e) = engine.save() {
                            tracing::warn!("Failed to save tag database: {}", e);
                        }
                    }

                    AppCommand::RemoveManualTag { path, tag } => {
                        let mut engine = tag_engine.lock().await;
                        engine.remove_manual_tag(&path.to_string_lossy(), &tag);
                        if let Err(e) = engine.save() {
                            tracing::warn!("Failed to save tag database: {}", e);
                        }
                    }

                    AppCommand::FilterByTag { tag, pane_id, path } => match tag {
                        // Clearing needs no listing.
                        None => {
                            let _ = event_tx.send(AppEvent::TagFilterApplied {
                                pane_id,
                                path,
                                tag: None,
                                paths: Vec::new(),
                            });
                        }
                        Some(tag_name) => {
                            let tag_engine = tag_engine.clone();
                            let vfs = vfs.clone();
                            tokio::spawn(async move {
                                match vfs.list_dir(&path).await {
                                    Ok(entries) => {
                                        let paths: Vec<RavenPath> = {
                                            let engine = tag_engine.lock().await;
                                            entries
                                                .iter()
                                                .filter(|e| engine.entry_has_tag(e, &tag_name))
                                                .map(|e| e.path.clone())
                                                .collect()
                                        };
                                        let _ = event_tx.send(AppEvent::TagFilterApplied {
                                            pane_id,
                                            path,
                                            tag: Some(tag_name),
                                            paths,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = event_tx.send(AppEvent::Notification {
                                            title: "Tag filter failed".to_string(),
                                            message: e.to_string(),
                                            level: raven_core::events::NotificationLevel::Error,
                                        });
                                    }
                                }
                            });
                        }
                    },

                    AppCommand::AnalyzeOrganization { path } => {
                        let vfs = vfs.clone();
                        tokio::spawn(async move {
                            match vfs.list_dir(&path).await {
                                Ok(entries) => {
                                    let config = OrganizeConfig::default();
                                    let suggestions = OrganizationAnalyzer::analyze(&entries, &config);
                                    let _ = event_tx.send(AppEvent::OrganizationAnalysisComplete {
                                        path,
                                        suggestions,
                                    });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AppEvent::Notification {
                                        title: "Organization analysis failed".to_string(),
                                        message: e.to_string(),
                                        level: raven_core::events::NotificationLevel::Error,
                                    });
                                }
                            }
                        });
                    }

                    AppCommand::ApplyOrganization { suggestions } => {
                        for suggestion in suggestions {
                            // Create the destination directory
                            let parent_path = {
                                let s_ref = &suggestion.source_files;
                                if let Some(first) = s_ref.first() {
                                    first.path.parent()
                                } else {
                                    None
                                }
                            };
                            if let Some(parent) = parent_path {
                                let dest = parent.join(&suggestion.destination.trim_end_matches('/'));
                                let _ = event_tx.send(AppEvent::Notification {
                                    title: "Organizing".to_string(),
                                    message: format!(
                                        "Moving {} files to {}",
                                        suggestion.source_files.len(),
                                        suggestion.destination
                                    ),
                                    level: raven_core::events::NotificationLevel::Info,
                                });

                                // Create directory
                                if let Some(local) = dest.as_local_path() {
                                    let _ = tokio::fs::create_dir_all(local).await;
                                }

                                // Move files
                                let sources: Vec<RavenPath> = suggestion
                                    .source_files
                                    .iter()
                                    .map(|f| f.path.clone())
                                    .collect();
                                let op_id = OperationId(
                                    next_op_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                                );
                                let op = Operation::new(
                                    op_id,
                                    OperationKind::Move,
                                    sources,
                                    Some(dest),
                                );
                                let executor = executor.clone();
                                let vfs = vfs.clone();
                                let event_tx = event_tx.clone();
                                tokio::spawn(async move {
                                    match executor.execute(&op, vfs.as_ref(), None).await {
                                        Ok(()) => {
                                            let _ = event_tx.send(AppEvent::OperationCompleted { id: op_id });
                                        }
                                        Err(e) => {
                                            let _ = event_tx.send(AppEvent::OperationFailed {
                                                id: op_id,
                                                error: e.to_string(),
                                            });
                                        }
                                    }
                                });
                            }
                        }
                    }

                    AppCommand::Quit => {
                        tracing::info!("Backend received quit command");
                        automation_engine.stop().await;
                        break;
                    }
                }
            }

            tracing::info!("Backend runtime shutting down");
        });
    });

    let app = RavenApplication::new(command_tx, event_rx);
    app.run()
}

/// List `path` and publish per-tag entry counts for `pane_id`.
///
/// Navigation computes counts from the entries it has already listed; this is the
/// path for refreshes that arrive without entries in hand (a tag toggle, a rule change).
async fn emit_tag_counts(
    vfs: &Arc<dyn VirtualFileSystem>,
    tag_engine: &Arc<tokio::sync::Mutex<TagEngine>>,
    event_tx: &tokio::sync::mpsc::UnboundedSender<AppEvent>,
    path: &RavenPath,
    pane_id: u32,
) {
    match vfs.list_dir(path).await {
        Ok(entries) => {
            let counts = {
                let engine = tag_engine.lock().await;
                engine.tag_counts(&entries)
            };
            let _ = event_tx.send(AppEvent::TagCountsUpdated { pane_id, counts });
        }
        Err(e) => {
            tracing::warn!("Tag count refresh failed for {}: {}", path, e);
        }
    }
}

fn sort_entries(entries: &mut Vec<FileEntry>, spec: &SortSpec) {
    entries.sort_by(|a, b| {
        // Directories first
        if spec.directories_first {
            let dir_ord = b.is_dir().cmp(&a.is_dir());
            if dir_ord != std::cmp::Ordering::Equal {
                return dir_ord;
            }
        }

        let ord = match spec.column {
            raven_core::sort::SortColumn::Name => {
                a.name.to_lowercase().cmp(&b.name.to_lowercase())
            }
            raven_core::sort::SortColumn::Size => a.metadata.size.cmp(&b.metadata.size),
            raven_core::sort::SortColumn::Modified => a.metadata.modified.cmp(&b.metadata.modified),
            raven_core::sort::SortColumn::Extension => {
                let a_ext = a.extension().unwrap_or("");
                let b_ext = b.extension().unwrap_or("");
                a_ext.cmp(b_ext)
            }
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };

        match spec.direction {
            raven_core::sort::SortDirection::Ascending => ord,
            raven_core::sort::SortDirection::Descending => ord.reverse(),
        }
    });
}

fn make_progress_reporter(
    _op_id: OperationId,
    event_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
) -> ProgressReporter {
    Arc::new(move |progress| {
        let _ = event_tx.send(AppEvent::OperationProgress { progress });
    })
}

/// Recursively calculate the total size of a directory's contents.
async fn calculate_dir_size(path: &std::path::Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![path.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut read_dir = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let ft = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_file() || ft.is_symlink() {
                if let Ok(meta) = entry.metadata().await {
                    total += meta.len();
                }
            } else if ft.is_dir() {
                stack.push(entry.path());
            }
        }
    }

    total
}

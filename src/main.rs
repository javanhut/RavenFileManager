use std::sync::Arc;

use raven_core::commands::{AppCommand, SearchMode};
use raven_core::entry::FileEntry;
use raven_core::events::AppEvent;
use raven_core::operations::{Operation, OperationId, OperationKind};
use raven_core::sort::SortSpec;
use raven_core::vfs::VirtualFileSystem;
use raven_ops::executor::{OperationExecutor, ProgressReporter};
use raven_ops::queue::OperationQueue;
use raven_ops::undo::UndoStack;
use raven_preview::router::PreviewRouter;
use raven_search::content::{ContentSearchConfig, ContentSearcher};
use raven_search::recursive::{RecursiveSearchConfig, RecursiveSearcher};
use raven_ui::app::RavenApplication;
use raven_vfs::router::VfsRouter;

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("Starting Raven File Manager");

    let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<AppCommand>();
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();

    // Spawn the Tokio runtime on a separate thread
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime");

        rt.block_on(async move {
            let vfs: Arc<dyn VirtualFileSystem> = Arc::new(VfsRouter::new());
            let queue = Arc::new(OperationQueue::new());
            let undo_stack = Arc::new(UndoStack::new());
            let executor = Arc::new(
                OperationExecutor::new(queue.clone(), undo_stack.clone())
                    .expect("Failed to create operation executor"),
            );
            let preview_router = Arc::new(PreviewRouter::new());
            let next_op_id = Arc::new(std::sync::atomic::AtomicU64::new(1));

            tracing::info!("Backend runtime started");

            while let Some(command) = command_rx.recv().await {
                let event_tx = event_tx.clone();
                let vfs = vfs.clone();
                let executor = executor.clone();
                let queue = queue.clone();
                let undo_stack = undo_stack.clone();
                let preview_router = preview_router.clone();
                let next_op_id = next_op_id.clone();

                match command {
                    // --- Navigation ---
                    AppCommand::Navigate { path, pane_id } => {
                        tokio::spawn(async move {
                            tracing::info!("Navigating to: {}", path);
                            match vfs.list_dir(&path).await {
                                Ok(mut entries) => {
                                    sort_entries(&mut entries, &SortSpec::default());
                                    let _ = event_tx.send(AppEvent::DirectoryLoaded {
                                        pane_id,
                                        path,
                                        entries,
                                    });
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

                    // --- File Operations ---
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

                        tokio::spawn(async move {
                            let reporter = make_progress_reporter(op_id, event_tx.clone());
                            match executor.execute(&op, vfs.as_ref(), Some(reporter)).await {
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

                        tokio::spawn(async move {
                            let reporter = make_progress_reporter(op_id, event_tx.clone());
                            match executor.execute(&op, vfs.as_ref(), Some(reporter)).await {
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
                            vec![path],
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
                                SearchMode::Content => {
                                    let searcher = ContentSearcher::new();
                                    let local_path = path.as_local_path().cloned().unwrap_or_default();
                                    let config = ContentSearchConfig::new(local_path, &query);
                                    match searcher.search(config) {
                                        Ok(mut rx) => {
                                            let mut count = 0u64;
                                            while let Some(m) = rx.recv().await {
                                                count += 1;
                                                // Create a FileEntry for each match
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
                        // Search cancellation is handled per-search via AtomicBool tokens
                        tracing::debug!("Search cancellation requested");
                    }

                    // --- Filter ---
                    AppCommand::SetFilter { filter, pane_id: _ } => {
                        // Filter is applied UI-side by re-filtering the current entries
                        // We send a notification back so the UI knows to update
                        let _ = event_tx.send(AppEvent::Notification {
                            title: "Filter applied".to_string(),
                            message: format!("Filter: {}", filter.query),
                            level: raven_core::events::NotificationLevel::Info,
                        });
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

                    // --- Refresh handled as Navigate ---
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

                    AppCommand::Quit => {
                        tracing::info!("Backend received quit command");
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

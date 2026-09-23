pub mod commands;
pub mod library;

use std::time::Duration;

use commands::AppState;
use library::{ExternalChangeMonitor, LibraryService};
use tauri::{Emitter, Manager};

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state_dir = app.path().app_data_dir()?;
            let service = LibraryService::new(state_dir)?;
            let state = AppState::new(service);
            let app_handle = app.handle().clone();
            let monitor = ExternalChangeMonitor::start(
                state.service_handle(),
                Duration::from_millis(350),
                Duration::from_millis(500),
                move |event| {
                    let _ = app_handle.emit("document-index-changed", event);
                },
            )?;
            state.set_external_change_monitor(monitor)?;

            // 接收目录补扫：打开资料库后周期性发现新文件；关闭应用即退出，无常驻进程。
            let receive_handle = app.handle().clone();
            let receive_monitor = commands::ReceiveDirectoryMonitor::start(
                state.service_handle(),
                Duration::from_secs(2),
                move |library, results| {
                    let _ = receive_handle.emit(
                        "receive-import-completed",
                        commands::ReceiveImportCompletedEvent { library, results },
                    );
                },
            )?;
            state.set_receive_directory_monitor(receive_monitor)?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::inspect_library_location,
            commands::create_library,
            commands::open_library,
            commands::import_document,
            commands::start_import,
            commands::resolve_import_item,
            commands::retry_import_item,
            commands::list_collections,
            commands::create_collection,
            commands::rename_collection,
            commands::move_collection,
            commands::delete_collection,
            commands::move_document_to_collection,
            commands::move_document_to_trash,
            commands::list_trash_documents,
            commands::restore_document,
            commands::permanently_delete_document,
            commands::empty_trash,
            commands::list_tags,
            commands::create_tag,
            commands::rename_tag,
            commands::delete_tag,
            commands::add_tag_to_document,
            commands::remove_tag_from_document,
            commands::update_document_metadata,
            commands::batch_organize_documents,
            commands::cancel_batch_document_operation,
            commands::list_documents,
            commands::search_documents,
            commands::index_pending_documents,
            commands::pending_index_count,
            commands::retry_document_index,
            commands::get_document_preview,
            commands::get_document_thumbnail,
            commands::list_document_sheets,
            commands::get_table_preview,
            commands::save_document_thumbnail,
            commands::open_document,
            commands::open_external_url,
            commands::list_document_format_capabilities,
            commands::list_recent_libraries,
            commands::forget_recent_library,
            commands::open_library_directory,
            commands::list_classification_rules,
            commands::apply_classification_rule_operation,
            commands::preview_classification,
            commands::list_receive_sources,
            commands::list_receive_source_candidates,
            commands::upsert_receive_source,
            commands::remove_receive_source,
            commands::list_receive_directory_files,
            commands::apply_receive_directory_selection,
            commands::skip_receive_directory_files,
            commands::scan_receive_sources,
            commands::list_receive_import_log,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run personal document manager");
}

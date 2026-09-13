pub mod commands;
pub mod library;

use std::time::Duration;

use commands::AppState;
use library::{ExternalChangeMonitor, LibraryService};
use tauri::{Emitter, Manager};

pub fn run() {
    tauri::Builder::default()
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
            commands::list_documents,
            commands::search_documents,
            commands::index_pending_documents,
            commands::retry_document_index,
            commands::get_document_preview,
            commands::get_document_thumbnail,
            commands::open_document,
            commands::list_recent_libraries,
            commands::forget_recent_library,
            commands::open_library_directory,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run personal document manager");
}

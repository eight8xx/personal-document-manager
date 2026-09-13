pub mod commands;
pub mod library;

use commands::AppState;
use library::LibraryService;
use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state_dir = app.path().app_data_dir()?;
            let service = LibraryService::new(state_dir)?;
            app.manage(AppState::new(service));
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

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
            commands::list_recent_libraries,
            commands::forget_recent_library,
            commands::open_library_directory,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run personal document manager");
}

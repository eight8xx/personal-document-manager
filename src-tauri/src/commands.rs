use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::library::{
    BootstrapState, DocumentSummary, LibraryError, LibraryResult, LibraryService, LibrarySummary,
    RecentLibrary,
};

pub struct AppState {
    service: Mutex<LibraryService>,
}

impl AppState {
    pub fn new(service: LibraryService) -> Self {
        Self {
            service: Mutex::new(service),
        }
    }

    fn service(&self) -> LibraryResult<MutexGuard<'_, LibraryService>> {
        self.service.lock().map_err(|_| LibraryError::StateLock)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl From<LibraryError> for CommandError {
    fn from(error: LibraryError) -> Self {
        Self {
            code: error.code().to_string(),
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryChangedEvent {
    pub action: String,
    pub library: LibrarySummary,
}

#[tauri::command]
pub fn bootstrap(state: State<'_, AppState>) -> Result<BootstrapState, CommandError> {
    bootstrap_contract(&state)
}

fn bootstrap_contract(state: &AppState) -> Result<BootstrapState, CommandError> {
    state.service()?.bootstrap().map_err(CommandError::from)
}

#[tauri::command]
pub fn inspect_library_location(
    path: String,
    state: State<'_, AppState>,
) -> Result<crate::library::LibraryLocationInspection, CommandError> {
    inspect_library_location_contract(&state, path)
}

fn inspect_library_location_contract(
    state: &AppState,
    path: String,
) -> Result<crate::library::LibraryLocationInspection, CommandError> {
    state
        .service()?
        .inspect_location(path)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn create_library(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LibrarySummary, CommandError> {
    let library = create_library_contract(&state, path)?;
    emit_library_changed(&app, "created", library.clone());
    Ok(library)
}

fn create_library_contract(state: &AppState, path: String) -> Result<LibrarySummary, CommandError> {
    state
        .service()?
        .create_library(path)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn open_library(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LibrarySummary, CommandError> {
    let library = open_library_contract(&state, path)?;
    emit_library_changed(&app, "opened", library.clone());
    Ok(library)
}

fn open_library_contract(state: &AppState, path: String) -> Result<LibrarySummary, CommandError> {
    state
        .service()?
        .open_library(path)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn import_document(
    path: String,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    import_document_contract(&state, path)
}

fn import_document_contract(
    state: &AppState,
    path: String,
) -> Result<DocumentSummary, CommandError> {
    state
        .service()?
        .import_document(path)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_documents(state: State<'_, AppState>) -> Result<Vec<DocumentSummary>, CommandError> {
    list_documents_contract(&state)
}

fn list_documents_contract(state: &AppState) -> Result<Vec<DocumentSummary>, CommandError> {
    state
        .service()?
        .list_documents()
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_recent_libraries(
    state: State<'_, AppState>,
) -> Result<Vec<RecentLibrary>, CommandError> {
    list_recent_libraries_contract(&state)
}

fn list_recent_libraries_contract(state: &AppState) -> Result<Vec<RecentLibrary>, CommandError> {
    Ok(state.service()?.list_recent_libraries())
}

#[tauri::command]
pub fn forget_recent_library(
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<RecentLibrary>, CommandError> {
    forget_recent_library_contract(&state, path)
}

fn forget_recent_library_contract(
    state: &AppState,
    path: String,
) -> Result<Vec<RecentLibrary>, CommandError> {
    state
        .service()?
        .forget_recent_library(path)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn open_library_directory(path: String) -> Result<(), CommandError> {
    open_library_directory_contract(path)
}

fn open_library_directory_contract(path: String) -> Result<(), CommandError> {
    crate::library::open_directory(path).map_err(CommandError::from)
}

fn emit_library_changed(app: &AppHandle, action: &str, library: LibrarySummary) {
    let event = LibraryChangedEvent {
        action: action.to_string(),
        library,
    };
    let _ = app.emit("library-changed", event);
}

#[cfg(test)]
mod tests {
    use super::{
        bootstrap_contract, create_library_contract, inspect_library_location_contract, AppState,
        CommandError, LibraryChangedEvent,
    };
    use crate::library::{LibraryService, LibrarySummary, LocationStatus};
    use tempfile::tempdir;

    #[test]
    fn create_and_bootstrap_commands_use_the_serialized_contract() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        let inspection =
            inspect_library_location_contract(&state, library_path.to_string_lossy().into_owned())
                .unwrap();
        assert_eq!(inspection.status, LocationStatus::Usable);

        let created =
            create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();
        let bootstrap = bootstrap_contract(&state).unwrap();
        assert_eq!(bootstrap.current_library.unwrap().id, created.id);
        assert_eq!(bootstrap.recent_libraries.len(), 1);
    }

    #[test]
    fn library_response_uses_camel_case() {
        let event = LibraryChangedEvent {
            action: "created".to_string(),
            library: LibrarySummary {
                id: "lib-1".to_string(),
                name: "资料库".to_string(),
                path: r"C:\Docs\Library".to_string(),
                created_at: "2026-09-13T00:00:00Z".to_string(),
            },
        };

        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["library"]["createdAt"], "2026-09-13T00:00:00Z");
        assert_eq!(value["action"], "created");
    }

    #[test]
    fn command_errors_serialize_as_structured_events() {
        let root = tempdir().unwrap();
        let non_empty = root.path().join("not-empty");
        std::fs::create_dir_all(&non_empty).unwrap();
        std::fs::write(non_empty.join("keep.txt"), "keep").unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());

        let error: CommandError =
            create_library_contract(&state, non_empty.to_string_lossy().into_owned()).unwrap_err();
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value["code"], "invalidLocation");
        assert!(value["message"].as_str().unwrap().contains("目录不为空"));
    }
}

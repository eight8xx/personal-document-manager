use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::library::{
    BootstrapState, CollectionDeleteResult, CollectionSummary, DocumentMetadataUpdate,
    DocumentPreview, DocumentSummary, DocumentThumbnail, ImportBatch, ImportDecision,
    ImportItemResult, ImportProgress, LibraryError, LibraryResult, LibraryService, LibrarySummary,
    RecentLibrary, TagSummary,
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
pub fn start_import(
    paths: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ImportBatch, CommandError> {
    start_import_contract(&state, paths, |progress| {
        let _ = app.emit("import-progress", progress);
    })
}

fn start_import_contract<F>(
    state: &AppState,
    paths: Vec<String>,
    on_progress: F,
) -> Result<ImportBatch, CommandError>
where
    F: FnMut(ImportProgress),
{
    let mut service = state.service()?;
    service
        .start_import_with_progress(paths, on_progress)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn resolve_import_item(
    item_id: String,
    decision: ImportDecision,
    state: State<'_, AppState>,
) -> Result<ImportItemResult, CommandError> {
    resolve_import_item_contract(&state, item_id, decision)
}

fn resolve_import_item_contract(
    state: &AppState,
    item_id: String,
    decision: ImportDecision,
) -> Result<ImportItemResult, CommandError> {
    state
        .service()?
        .resolve_import_item(&item_id, decision)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn retry_import_item(
    item_id: String,
    state: State<'_, AppState>,
) -> Result<ImportItemResult, CommandError> {
    retry_import_item_contract(&state, item_id)
}

fn retry_import_item_contract(
    state: &AppState,
    item_id: String,
) -> Result<ImportItemResult, CommandError> {
    state
        .service()?
        .retry_import_item(&item_id)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_collections(
    state: State<'_, AppState>,
) -> Result<Vec<CollectionSummary>, CommandError> {
    list_collections_contract(&state)
}

fn list_collections_contract(state: &AppState) -> Result<Vec<CollectionSummary>, CommandError> {
    state
        .service()?
        .list_collections()
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn create_collection(
    name: String,
    parent_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<CollectionSummary, CommandError> {
    create_collection_contract(&state, name, parent_id)
}

fn create_collection_contract(
    state: &AppState,
    name: String,
    parent_id: Option<String>,
) -> Result<CollectionSummary, CommandError> {
    state
        .service()?
        .create_collection(name, parent_id)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn rename_collection(
    collection_id: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<CollectionSummary, CommandError> {
    rename_collection_contract(&state, collection_id, name)
}

fn rename_collection_contract(
    state: &AppState,
    collection_id: String,
    name: String,
) -> Result<CollectionSummary, CommandError> {
    state
        .service()?
        .rename_collection(&collection_id, name)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn move_collection(
    collection_id: String,
    parent_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<CollectionSummary, CommandError> {
    move_collection_contract(&state, collection_id, parent_id)
}

fn move_collection_contract(
    state: &AppState,
    collection_id: String,
    parent_id: Option<String>,
) -> Result<CollectionSummary, CommandError> {
    state
        .service()?
        .move_collection(&collection_id, parent_id)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn delete_collection(
    collection_id: String,
    state: State<'_, AppState>,
) -> Result<CollectionDeleteResult, CommandError> {
    delete_collection_contract(&state, collection_id)
}

fn delete_collection_contract(
    state: &AppState,
    collection_id: String,
) -> Result<CollectionDeleteResult, CommandError> {
    state
        .service()?
        .delete_collection(&collection_id)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn move_document_to_collection(
    document_id: String,
    collection_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    move_document_to_collection_contract(&state, document_id, collection_id)
}

fn move_document_to_collection_contract(
    state: &AppState,
    document_id: String,
    collection_id: String,
) -> Result<DocumentSummary, CommandError> {
    state
        .service()?
        .move_document_to_collection(&document_id, &collection_id)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_tags(state: State<'_, AppState>) -> Result<Vec<TagSummary>, CommandError> {
    list_tags_contract(&state)
}

fn list_tags_contract(state: &AppState) -> Result<Vec<TagSummary>, CommandError> {
    state.service()?.list_tags().map_err(CommandError::from)
}

#[tauri::command]
pub fn create_tag(name: String, state: State<'_, AppState>) -> Result<TagSummary, CommandError> {
    create_tag_contract(&state, name)
}

fn create_tag_contract(state: &AppState, name: String) -> Result<TagSummary, CommandError> {
    state
        .service()?
        .create_tag(name)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn rename_tag(
    tag_id: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<TagSummary, CommandError> {
    rename_tag_contract(&state, tag_id, name)
}

fn rename_tag_contract(
    state: &AppState,
    tag_id: String,
    name: String,
) -> Result<TagSummary, CommandError> {
    state
        .service()?
        .rename_tag(&tag_id, name)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn delete_tag(tag_id: String, state: State<'_, AppState>) -> Result<(), CommandError> {
    delete_tag_contract(&state, tag_id)
}

fn delete_tag_contract(state: &AppState, tag_id: String) -> Result<(), CommandError> {
    state
        .service()?
        .delete_tag(&tag_id)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn add_tag_to_document(
    document_id: String,
    tag_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    add_tag_to_document_contract(&state, document_id, tag_id)
}

fn add_tag_to_document_contract(
    state: &AppState,
    document_id: String,
    tag_id: String,
) -> Result<DocumentSummary, CommandError> {
    state
        .service()?
        .add_tag_to_document(&document_id, &tag_id)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn remove_tag_from_document(
    document_id: String,
    tag_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    remove_tag_from_document_contract(&state, document_id, tag_id)
}

fn remove_tag_from_document_contract(
    state: &AppState,
    document_id: String,
    tag_id: String,
) -> Result<DocumentSummary, CommandError> {
    state
        .service()?
        .remove_tag_from_document(&document_id, &tag_id)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn update_document_metadata(
    document_id: String,
    update: DocumentMetadataUpdate,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    update_document_metadata_contract(&state, document_id, update)
}

fn update_document_metadata_contract(
    state: &AppState,
    document_id: String,
    update: DocumentMetadataUpdate,
) -> Result<DocumentSummary, CommandError> {
    state
        .service()?
        .update_document_metadata(&document_id, update)
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
pub fn get_document_preview(
    document_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentPreview, CommandError> {
    get_document_preview_contract(&state, document_id)
}

fn get_document_preview_contract(
    state: &AppState,
    document_id: String,
) -> Result<DocumentPreview, CommandError> {
    state
        .service()?
        .get_document_preview(&document_id)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_document_thumbnail(
    document_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentThumbnail, CommandError> {
    get_document_thumbnail_contract(&state, document_id)
}

fn get_document_thumbnail_contract(
    state: &AppState,
    document_id: String,
) -> Result<DocumentThumbnail, CommandError> {
    state
        .service()?
        .get_document_thumbnail(&document_id)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn open_document(document_id: String, state: State<'_, AppState>) -> Result<(), CommandError> {
    open_document_contract(&state, document_id)
}

fn open_document_contract(state: &AppState, document_id: String) -> Result<(), CommandError> {
    state
        .service()?
        .open_document(&document_id)
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
        add_tag_to_document_contract, bootstrap_contract, create_collection_contract,
        create_library_contract, create_tag_contract, delete_collection_contract,
        delete_tag_contract, inspect_library_location_contract, list_collections_contract,
        list_tags_contract, move_collection_contract, move_document_to_collection_contract,
        remove_tag_from_document_contract, rename_collection_contract, rename_tag_contract,
        resolve_import_item_contract, retry_import_item_contract, start_import_contract,
        update_document_metadata_contract, AppState, CommandError, LibraryChangedEvent,
    };
    use crate::library::{
        DocumentMetadataUpdate, DocumentPreview, DocumentThumbnail, ImportDecision,
        ImportItemStatus, LibraryService, LibrarySummary, LocationStatus,
    };
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
    fn preview_and_thumbnail_responses_use_camel_case() {
        let preview = serde_json::to_value(DocumentPreview::Pdf {
            data_url: "data:application/pdf;base64,AA==".to_string(),
            page_count: Some(3),
        })
        .unwrap();
        assert_eq!(preview["kind"], "pdf");
        assert!(preview.get("dataUrl").is_some());
        assert!(preview.get("pageCount").is_some());
        assert!(preview.get("data_url").is_none());

        let thumbnail = serde_json::to_value(DocumentThumbnail::Fallback {
            reason: "使用类型图标。".to_string(),
        })
        .unwrap();
        assert_eq!(thumbnail["kind"], "fallback");
        assert!(thumbnail.get("reason").is_some());
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

    #[test]
    fn import_commands_use_camel_case_contract_and_emit_progress() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();
        let source_path = root.path().join("source.txt");
        std::fs::write(&source_path, "contract contents").unwrap();

        let mut progress = Vec::new();
        let batch = start_import_contract(
            &state,
            vec![source_path.to_string_lossy().into_owned()],
            |event| progress.push(event),
        )
        .unwrap();
        assert_eq!(progress.first().unwrap().completed, 0);
        assert!(!progress.first().unwrap().finished);
        assert!(progress.last().unwrap().finished);
        assert_eq!(progress.last().unwrap().completed, 1);

        let value = serde_json::to_value(&batch).unwrap();
        assert!(value.get("batchId").is_some());
        assert!(value.get("items").is_some());
        assert!(value.get("importedCount").is_some());
        assert!(value.get("duplicateCount").is_some());
        assert!(value.get("sourceChangedCount").is_some());
        assert!(value.get("failedCount").is_some());
        assert!(value.get("ignoredCount").is_some());
        assert!(value.get("imported_count").is_none());
        assert_eq!(value["items"][0]["status"], "imported");
        assert!(value["items"][0].get("itemId").is_some());
        assert!(value["items"][0].get("sourcePath").is_some());
        assert!(value["items"][0].get("fileName").is_some());
        assert!(value["items"][0].get("fileType").is_some());
        assert!(value["items"][0].get("documentId").is_some());
        assert!(value["items"][0].get("duplicateDocumentId").is_some());
        assert!(value["items"][0].get("errorStage").is_some());
        assert!(value["items"][0].get("errorMessage").is_some());

        let duplicate = start_import_contract(
            &state,
            vec![source_path.to_string_lossy().into_owned()],
            |_| {},
        )
        .unwrap()
        .items
        .remove(0);
        let resolved =
            resolve_import_item_contract(&state, duplicate.item_id, ImportDecision::UseExisting)
                .unwrap();
        assert_eq!(resolved.status, ImportItemStatus::Skipped);
        assert_eq!(
            serde_json::to_value(ImportDecision::UseExisting).unwrap(),
            "useExisting"
        );

        let missing_path = root.path().join("retry.txt");
        let failed = start_import_contract(
            &state,
            vec![missing_path.to_string_lossy().into_owned()],
            |_| {},
        )
        .unwrap()
        .items
        .remove(0);
        std::fs::write(&missing_path, "retried").unwrap();
        let retried = retry_import_item_contract(&state, failed.item_id).unwrap();
        let value = serde_json::to_value(retried).unwrap();
        assert_eq!(value["status"], "imported");
        assert!(value.get("retryable").is_some());
    }

    #[test]
    fn collection_commands_use_camel_case_contract() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();

        let collections = list_collections_contract(&state).unwrap();
        assert_eq!(collections[0].id, "inbox");
        let parent = create_collection_contract(&state, "Parent".to_string(), None).unwrap();
        let child =
            create_collection_contract(&state, "Child".to_string(), Some(parent.id.clone()))
                .unwrap();
        let renamed =
            rename_collection_contract(&state, child.id.clone(), "Renamed".to_string()).unwrap();
        let moved = move_collection_contract(&state, renamed.id.clone(), None).unwrap();

        let source_path = root.path().join("document.txt");
        std::fs::write(&source_path, "document").unwrap();
        let imported = start_import_contract(
            &state,
            vec![source_path.to_string_lossy().into_owned()],
            |_| {},
        )
        .unwrap()
        .items
        .remove(0);
        let document = move_document_to_collection_contract(
            &state,
            imported.document_id.unwrap(),
            moved.id.clone(),
        )
        .unwrap();
        let document_value = serde_json::to_value(document).unwrap();
        assert_eq!(document_value["collectionId"], moved.id);

        let collection_value = serde_json::to_value(&moved).unwrap();
        assert!(collection_value.get("parentId").is_some());
        assert!(collection_value.get("isInbox").is_some());
        assert!(collection_value.get("documentCount").is_some());

        let deleted = delete_collection_contract(&state, moved.id).unwrap();
        let delete_value = serde_json::to_value(deleted).unwrap();
        assert!(delete_value.get("collectionId").is_some());
        assert!(delete_value.get("targetCollectionId").is_some());
        assert!(delete_value.get("movedDocumentCount").is_some());
    }

    #[test]
    fn tag_and_metadata_commands_use_camel_case_contract() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();
        let source_path = root.path().join("document.txt");
        std::fs::write(&source_path, "document").unwrap();
        let document_id = start_import_contract(
            &state,
            vec![source_path.to_string_lossy().into_owned()],
            |_| {},
        )
        .unwrap()
        .items
        .remove(0)
        .document_id
        .unwrap();

        let work = create_tag_contract(&state, "工作".to_string()).unwrap();
        let important = create_tag_contract(&state, "重要".to_string()).unwrap();
        let renamed = rename_tag_contract(&state, work.id.clone(), "项目".to_string()).unwrap();
        assert_eq!(renamed.name, "项目");
        assert_eq!(list_tags_contract(&state).unwrap().len(), 2);

        let updated = update_document_metadata_contract(
            &state,
            document_id.clone(),
            DocumentMetadataUpdate {
                title: "项目文档".to_string(),
                description: Some("说明".to_string()),
                document_date: Some("2025-02-03".to_string()),
                collection_id: "inbox".to_string(),
                tag_ids: vec![renamed.id.clone(), important.id.clone()],
            },
        )
        .unwrap();
        let value = serde_json::to_value(updated).unwrap();
        assert_eq!(value["documentDate"], "2025-02-03");
        assert_eq!(value["tags"].as_array().unwrap().len(), 2);
        assert!(value.get("sourcePath").is_some());

        let removed =
            remove_tag_from_document_contract(&state, document_id.clone(), important.id.clone())
                .unwrap();
        assert_eq!(removed.tags.len(), 1);
        let added =
            add_tag_to_document_contract(&state, document_id, important.id.clone()).unwrap();
        assert_eq!(added.tags.len(), 2);
        delete_tag_contract(&state, important.id).unwrap();
        assert_eq!(list_tags_contract(&state).unwrap().len(), 1);
    }
}

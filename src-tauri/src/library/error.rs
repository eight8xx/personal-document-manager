use std::io;

use thiserror::Error;

pub type LibraryResult<T> = Result<T, LibraryError>;

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("{0}")]
    InvalidLocation(String),
    #[error("{0}")]
    InvalidLibrary(String),
    #[error("{0}")]
    RecentLibrary(String),
    #[error("无法访问文件系统：{0}")]
    Io(#[from] io::Error),
    #[error("资料库数据库错误：{0}")]
    Database(#[from] rusqlite::Error),
    #[error("资料库元数据格式错误：{0}")]
    Json(#[from] serde_json::Error),
    #[error("请先打开资料库。")]
    NoCurrentLibrary,
    #[error("{0}")]
    UnsupportedFile(String),
    #[error("{0}")]
    ImportFile(String),
    #[error("{0}")]
    ImportItemNotFound(String),
    #[error("{0}")]
    InvalidImportDecision(String),
    #[error("{0}")]
    InvalidCollection(String),
    #[error("{0}")]
    CollectionNotFound(String),
    #[error("{0}")]
    ProtectedCollection(String),
    #[error("{0}")]
    CollectionCycle(String),
    #[error("无法打开目录：{0}")]
    OpenDirectory(String),
    #[error("资料库状态锁已损坏")]
    StateLock,
}

impl LibraryError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidLocation(_) => "invalidLocation",
            Self::InvalidLibrary(_) => "invalidLibrary",
            Self::RecentLibrary(_) => "recentLibrary",
            Self::Io(_) => "filesystem",
            Self::Database(_) => "database",
            Self::Json(_) => "invalidMetadata",
            Self::NoCurrentLibrary => "noCurrentLibrary",
            Self::UnsupportedFile(_) => "unsupportedFile",
            Self::ImportFile(_) => "importFile",
            Self::ImportItemNotFound(_) => "importItemNotFound",
            Self::InvalidImportDecision(_) => "invalidImportDecision",
            Self::InvalidCollection(_) => "invalidCollection",
            Self::CollectionNotFound(_) => "collectionNotFound",
            Self::ProtectedCollection(_) => "protectedCollection",
            Self::CollectionCycle(_) => "collectionCycle",
            Self::OpenDirectory(_) => "openDirectory",
            Self::StateLock => "stateLock",
        }
    }
}

use axum::Json;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::website;

#[derive(Deserialize)]
pub struct ReadRequest {
    path: String,
}

pub async fn read(Json(req): Json<ReadRequest>) -> Json<String> {
    match fs::read_to_string(req.path) {
        Ok(content) => Json(content),
        Err(_) => Json("Error reading file".into()),
    }
}

#[derive(Deserialize)]
pub struct WriteRequest {
    path: String,
    content: String,
}

pub async fn write(Json(req): Json<WriteRequest>) -> Json<String> {
    match fs::write(req.path, req.content) {
        Ok(_) => Json("Written".into()),
        Err(_) => Json("Error writing file".into()),
    }
}

#[derive(Deserialize, Default)]
pub struct DirectoryListRequest {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
pub struct DirectoryCreateRequest {
    parent_path: String,
    name: String,
}

#[derive(Serialize, Clone)]
pub struct DirectoryEntry {
    name: String,
    path: String,
    modified_ms: u128,
    permissions: String,
}

#[derive(Serialize)]
pub struct DirectoryListResponse {
    status: bool,
    message: String,
    root: String,
    current: String,
    parent: Option<String>,
    entries: Vec<DirectoryEntry>,
}

#[derive(Serialize)]
pub struct DirectoryCreateResponse {
    status: bool,
    message: String,
    path: String,
}

pub async fn list_directories(
    Json(req): Json<DirectoryListRequest>,
) -> Json<DirectoryListResponse> {
    match collect_directories(&req.path) {
        Ok(response) => Json(response),
        Err(error) => Json(DirectoryListResponse {
            status: false,
            message: error,
            root: String::new(),
            current: String::new(),
            parent: None,
            entries: Vec::new(),
        }),
    }
}

pub async fn create_directory(
    Json(req): Json<DirectoryCreateRequest>,
) -> Json<DirectoryCreateResponse> {
    match create_website_directory(&req.parent_path, &req.name) {
        Ok(path) => Json(DirectoryCreateResponse {
            status: true,
            message: "Directory created".to_string(),
            path: path.display().to_string(),
        }),
        Err(error) => Json(DirectoryCreateResponse {
            status: false,
            message: error,
            path: String::new(),
        }),
    }
}

fn collect_directories(path: &str) -> Result<DirectoryListResponse, String> {
    let (root, current) = resolve_directory_picker_path(path)?;
    let mut entries = fs::read_dir(&current)
        .map_err(|error| format!("Failed to read directory: {error}"))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = entry.metadata().ok()?;
            if !metadata.is_dir() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let modified_ms = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis())
                .unwrap_or_default();
            let permissions = if metadata.permissions().readonly() {
                "555 / www"
            } else {
                "755 / www"
            }
            .to_string();
            Some(DirectoryEntry {
                name,
                path: path.display().to_string(),
                modified_ms,
                permissions,
            })
        })
        .collect::<Vec<_>>();

    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });

    let parent = current
        .parent()
        .filter(|parent| path_starts_with(parent, &root) && *parent != root)
        .map(|parent| parent.display().to_string());

    Ok(DirectoryListResponse {
        status: true,
        message: String::new(),
        root: root.display().to_string(),
        current: current.display().to_string(),
        parent,
        entries,
    })
}

fn create_website_directory(parent_path: &str, name: &str) -> Result<PathBuf, String> {
    let (_, parent) = resolve_directory_picker_path(parent_path)?;
    let name = name.trim();
    if name.is_empty() {
        return Err("Please enter a directory name".to_string());
    }
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err("Directory name contains unsupported characters".to_string());
    }

    let path = parent.join(name);
    if path.exists() {
        return Err("Directory already exists".to_string());
    }
    fs::create_dir_all(&path).map_err(|error| format!("Failed to create directory: {error}"))?;
    Ok(path)
}

fn resolve_directory_picker_path(path: &str) -> Result<(PathBuf, PathBuf), String> {
    let root = website::resolve_website_root();
    fs::create_dir_all(&root)
        .map_err(|error| format!("Failed to create website root: {error}"))?;
    let root = fs::canonicalize(&root)
        .map_err(|error| format!("Failed to resolve website root: {error}"))?;
    let requested = path.trim();
    let target = if requested.is_empty() {
        root.clone()
    } else {
        let path = PathBuf::from(requested);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    };
    let target = fs::canonicalize(&target)
        .map_err(|error| format!("Failed to resolve selected directory: {error}"))?;
    if !target.is_dir() {
        return Err("Selected path is not a directory".to_string());
    }
    if !path_starts_with(&target, &root) {
        return Err("Selected directory must stay inside the website root".to_string());
    }
    Ok((root, target))
}

fn path_starts_with(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

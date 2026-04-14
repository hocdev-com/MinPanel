use axum::Json;
use serde::Deserialize;
use std::fs;

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

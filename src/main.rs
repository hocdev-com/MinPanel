#![cfg_attr(windows, windows_subsystem = "windows")]

mod auth;
mod dashboard;
mod file;
mod models;
mod process;
mod routes;
mod system;
mod website;
#[cfg(windows)]
mod windows_gui;

use axum::Router;
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
};
use std::{env, io, net::SocketAddr};
use tower_http::cors::CorsLayer;

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_gui::launch(preferred_port()) {
        windows_gui::show_startup_error("MinPanel", &error);
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
#[tokio::main]
async fn main() {
    if let Err(error) = run_server_foreground(preferred_port()).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

pub(crate) fn preferred_port() -> u16 {
    env::var("MINI_PANEL_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8080)
}

pub(crate) fn app_router() -> Router {
    Router::new()
        .merge(routes::routes())
        .route(
            "/assets/dashboard/styles.css",
            axum::routing::get(dashboard_styles),
        )
        .route(
            "/assets/dashboard/icons.css",
            axum::routing::get(dashboard_icons),
        )
        .route(
            "/assets/dashboard/app.js",
            axum::routing::get(dashboard_script),
        )
        .route("/favicon.ico", axum::routing::get(dashboard_favicon_ico))
        .route("/assets/dashboard/favicon.png", axum::routing::get(dashboard_favicon))
        .layer(CorsLayer::permissive())
}

#[cfg(not(windows))]
async fn run_server_foreground(port: u16) -> Result<(), String> {
    let listener = bind_listener(port).await?;
    let addr = listener
        .local_addr()
        .map_err(|error| format!("Failed to read bound listener address: {error}"))?;
    let port = addr.port();

    println!("Server listening on http://{}", addr);
    println!("Local access: http://127.0.0.1:{port}");
    println!("Local access: http://localhost:{port}");

    axum::serve(listener, app_router())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| format!("MinPanel server exited with error: {error}"))?;

    Ok(())
}

#[cfg(not(windows))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("Shutting down MinPanel...");
    match stop_runtimes_on_shutdown().await {
        Ok(()) => println!("Background runtimes stopped."),
        Err(error) => eprintln!("Failed to stop background runtimes cleanly: {error}"),
    }
}

pub(crate) async fn stop_runtimes_on_shutdown() -> Result<(), String> {
    match tokio::task::spawn_blocking(dashboard::stop_all_runtimes_fast).await {
        Ok(result) => result,
        Err(error) => Err(format!("Failed to join runtime shutdown task: {error}")),
    }
}

async fn dashboard_styles() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/css; charset=utf-8"),
        )],
        include_str!("ui/dashboard/styles.css"),
    )
}

async fn dashboard_icons() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/css; charset=utf-8"),
        )],
        include_str!("ui/dashboard/icons.css"),
    )
}

async fn dashboard_script() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/javascript; charset=utf-8"),
        )],
        include_str!("ui/dashboard/app.js"),
    )
}

async fn dashboard_favicon() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("image/png"),
        )],
        include_bytes!("ui/dashboard/favicon.png").as_slice(),
    )
}

async fn dashboard_favicon_ico() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("image/x-icon"),
        )],
        include_bytes!("ui/dashboard/favicon.png").as_slice(),
    )
}

pub(crate) async fn bind_listener(preferred_port: u16) -> Result<tokio::net::TcpListener, String> {
    let mut port = preferred_port;
    let max_port = preferred_port.saturating_add(100);

    loop {
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                if port >= max_port {
                    return Err(format!(
                        "MinPanel cannot start because ports {preferred_port}-{port} are already in use. Stop existing instances or set MINI_PANEL_PORT to a different fixed port."
                    ));
                }
                port += 1;
            }
            Err(error) => return Err(format!("MinPanel failed to bind {}: {error}", addr)),
        }
    }
}

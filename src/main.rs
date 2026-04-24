#![cfg_attr(windows, windows_subsystem = "windows")]

mod auth;
mod dashboard;
mod file;
mod process;
mod routes;
mod system;
mod website;
#[cfg(windows)]
mod windows_gui;

use axum::Router;
use axum::{
    extract::DefaultBodyLimit,
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
};
use std::{env, io, net::SocketAddr};
use tower_http::cors::{AllowOrigin, CorsLayer};

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
    auth::initialize();

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
        .route(
            "/assets/dashboard/favicon.png",
            axum::routing::get(dashboard_favicon),
        )
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024)) // 10 MB
        .layer(localhost_cors_layer())
}

fn localhost_cors_layer() -> CorsLayer {
    use axum::http::Method;

    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            let Ok(origin_str) = origin.to_str() else {
                return false;
            };
            let lower = origin_str.to_ascii_lowercase();
            lower.starts_with("http://localhost")
                || lower.starts_with("https://localhost")
                || lower.starts_with("http://127.0.0.1")
                || lower.starts_with("https://127.0.0.1")
        }))
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::COOKIE])
        .allow_credentials(true)
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
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/css; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
        ],
        include_str!("ui/dashboard/styles.css"),
    )
}

async fn dashboard_icons() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/css; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
        ],
        include_str!("ui/dashboard/icons.css"),
    )
}

async fn dashboard_script() -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/javascript; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
        ],
        include_str!("ui/dashboard/app.js"),
    )
}

async fn dashboard_favicon() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("image/svg+xml"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400"),
            ),
        ],
        include_str!("ui/dashboard/favicon.svg"),
    )
}

async fn dashboard_favicon_ico() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("image/svg+xml"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400"),
            ),
        ],
        include_str!("ui/dashboard/favicon.svg"),
    )
}

pub(crate) fn preferred_bind_address() -> [u8; 4] {
    match env::var("MINI_PANEL_BIND") {
        Ok(ref value) if value.trim() == "0.0.0.0" => [0, 0, 0, 0],
        _ => [127, 0, 0, 1],
    }
}

pub(crate) async fn bind_listener(preferred_port: u16) -> Result<tokio::net::TcpListener, String> {
    let mut port = preferred_port;
    let max_port = preferred_port.saturating_add(100);
    let bind_addr = preferred_bind_address();

    loop {
        let addr = SocketAddr::from((bind_addr, port));
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

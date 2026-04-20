use axum::{
    response::Redirect,
    routing::{get, post},
    Router,
};

use crate::{auth, dashboard, file, process, system, website};

async fn root_redirect() -> Redirect {
    Redirect::permanent("/dashboard")
}

async fn overview_redirect() -> Redirect {
    Redirect::permanent("/website")
}

pub fn routes() -> Router {
    Router::new()
        .route("/", get(root_redirect))
        .route("/dashboard", get(dashboard::page))
        .route("/website", get(website::website_page))
        .route("/overview", get(overview_redirect))
        .route("/software", get(dashboard::software_page))
        .route("/traffic", get(dashboard::page))
        .route("/disks", get(dashboard::page))
        .route("/processes", get(dashboard::page))
        .route("/dashboard/data", get(dashboard::data))
        .route("/software/refresh", post(dashboard::refresh_software_store))
        .route(
            "/software/install",
            post(dashboard::install_software_package),
        )
        .route(
            "/software/download",
            post(dashboard::download_software_package),
        )
        .route("/software/start", post(dashboard::start_software_package))
        .route("/software/stop", post(dashboard::stop_software_package))
        .route(
            "/software/restart",
            post(dashboard::restart_software_package),
        )
        .route("/software/reload", post(dashboard::reload_software_package))
        .route(
            "/software/open-path",
            post(dashboard::open_software_install_path),
        )
        .route(
            "/software/uninstall",
            post(dashboard::uninstall_software_package),
        )
        .route("/tasks", get(dashboard::list_tasks))
        .route("/tasks/:id/log", get(dashboard::get_task_log))
        .route(
            "/website/php-binding",
            post(website::save_website_php_binding),
        )
        .route("/website/create", post(website::create_website_site))
        .route("/website/delete", post(website::delete_website_site))
        .route("/website/start", post(website::start_website_site))
        .route("/website/pause", post(website::pause_website_site))
        .route("/website/ssl", post(website::apply_website_ssl_handler))
        .route("/login", post(auth::login))
        .route("/system", get(system::info))
        .route("/process", get(process::list))
        .route("/process/kill", post(process::kill))
        .route("/files/read", post(file::read))
        .route("/files/write", post(file::write))
}

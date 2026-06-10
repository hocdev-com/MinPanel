pub(crate) fn get_shared_ui_asset(relative_path: &str) -> Option<&'static str> {
    match relative_path {
        "core.js" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/ui/shared/core.js"
        ))),
        "login.html" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/ui/shared/login.html"
        ))),
        "pages/dashboard.js" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/ui/shared/pages/dashboard.js"
        ))),
        "pages/software.js" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/ui/shared/pages/software.js"
        ))),
        "pages/database.js" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/ui/shared/pages/database.js"
        ))),
        "pages/website.js" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/ui/shared/pages/website.js"
        ))),
        "pages/file.js" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/ui/shared/pages/file.js"
        ))),
        _ => None,
    }
}

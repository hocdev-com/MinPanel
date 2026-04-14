use axum::Json;
use sysinfo::System;

pub async fn info() -> Json<serde_json::Value> {
    let mut sys = System::new_all();
    sys.refresh_all();

    let cpu = sys.global_cpu_info().cpu_usage();

    Json(serde_json::json!({
        "cpu_usage": cpu,
        "total_memory": sys.total_memory(),
        "used_memory": sys.used_memory()
    }))
}

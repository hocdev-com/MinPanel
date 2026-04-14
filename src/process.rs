use axum::Json;
use serde::Deserialize;
use sysinfo::{Pid, System};

pub async fn list() -> Json<Vec<String>> {
    let mut sys = System::new_all();
    sys.refresh_all();

    let processes = sys
        .processes()
        .iter()
        .map(|(pid, proc_)| format!("{}: {}", pid, proc_.name()))
        .collect();

    Json(processes)
}

#[derive(Deserialize)]
pub struct KillRequest {
    pid: i32,
}

pub async fn kill(Json(payload): Json<KillRequest>) -> Json<String> {
    let mut sys = System::new_all();
    sys.refresh_all();

    if let Some(proc_) = sys.process(Pid::from(payload.pid as usize)) {
        let _ = proc_.kill();
        Json("Killed".into())
    } else {
        Json("Not found".into())
    }
}

use axum::{
    body::Bytes,
    extract::{OriginalUri, Query},
    http::{header, HeaderMap, Method, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};
use mlua::Lua;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    cmp::Ordering,
    collections::hash_map::DefaultHasher,
    collections::{HashMap, HashSet},
    env, fs,
    hash::{Hash, Hasher},
    io::Write,
    net::{IpAddr, SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime},
};
use sysinfo::{Disks, Networks, ProcessRefreshKind, System};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::website;

#[cfg(not(windows))]
use std::net::TcpStream;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

const SOFTWARE_CACHE_TTL: Duration = Duration::from_secs(3600);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
static SOFTWARE_VIEW_CACHE: OnceLock<Mutex<Option<CachedSoftwareView>>> = OnceLock::new();
static SOFTWARE_REFRESH_STATE: OnceLock<Mutex<SoftwareRefreshState>> = OnceLock::new();
static TASK_MANAGER: OnceLock<Mutex<HashMap<String, TaskInfo>>> = OnceLock::new();

#[derive(Serialize, Clone, Debug)]
pub struct TaskInfo {
    pub id: String,
    pub name: String,
    pub status: String, // "pending", "running", "success", "failed"
    pub log: String,
    pub last_message: String,
    pub created_at: u64,
}

fn get_task_manager() -> &'static Mutex<HashMap<String, TaskInfo>> {
    TASK_MANAGER.get_or_init(|| Mutex::new(HashMap::new()))
}

fn update_task_log(task_id: &str, text: &str) {
    if let Ok(mut tasks) = get_task_manager().lock() {
        if let Some(task) = tasks.get_mut(task_id) {
            task.log.push_str(text);
            task.log.push('\n');
            task.last_message = text.to_string();
        }
    }
}

fn set_task_status(task_id: &str, status: &str) {
    if let Ok(mut tasks) = get_task_manager().lock() {
        if let Some(task) = tasks.get_mut(task_id) {
            task.status = status.to_string();
        }
    }
}

#[derive(Serialize)]
pub struct DashboardData {
    hostname: String,
    primary_ip: String,
    os_name: String,
    kernel_version: String,
    uptime: u64,
    cpu_usage: f32,
    cpu_brand: String,
    cpu_frequency: u64,
    cpu_cores: usize,
    total_memory: u64,
    used_memory: u64,
    real_used_memory: u64,
    free_memory: u64,
    buffered_memory: u64,
    cached_memory: u64,
    total_swap: u64,
    used_swap: u64,
    app_disk: Option<DiskData>,
    load_avg: LoadAverageData,
    process_count: usize,
    site_count: usize,
    ftp_count: usize,
    database_count: usize,
    warning_count: usize,
    websites: Vec<website::WebsiteEntry>,
    php_runtimes: Vec<PhpRuntimeOption>,
    web_server: Option<WebServerRuntime>,
    software_types: Vec<SoftwareTypeEntry>,
    software_plugins: Vec<SoftwarePluginEntry>,
    workspace_root: String,
    website_root: String,
    databases: Vec<DatabaseEntry>,
    disks: Vec<DiskData>,
    networks: Vec<NetworkData>,
    top_processes: Vec<ProcessData>,
    alerts: Vec<String>,
}

#[derive(Serialize)]
pub struct LoadAverageData {
    one: f64,
    five: f64,
    fifteen: f64,
    max: f64,
    safe: f64,
}

#[derive(Serialize, Clone)]
pub struct DiskData {
    name: String,
    mount_point: String,
    total_space: u64,
    available_space: u64,
}

#[derive(Serialize)]
pub struct NetworkData {
    name: String,
    received: u64,
    transmitted: u64,
    total_received: u64,
    total_transmitted: u64,
}

#[derive(Serialize)]
pub struct ProcessData {
    pid: u32,
    name: String,
    cpu_usage: f32,
    memory: u64,
    status: String,
}

#[derive(Serialize, Clone)]
pub struct DatabaseEntry {
    id: String,
    name: String,
    engine: String,
    username: String,
    password: String,
    permission: String,
    backup_count: usize,
    size: u64,
    modified_ms: u128,
    path: String,
    source: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct DatabaseCredentialStore {
    mysql: HashMap<String, StoredDatabaseCredential>,
}

#[derive(Serialize, Deserialize, Clone)]
struct StoredDatabaseCredential {
    username: String,
    password: String,
}

pub struct LuaPluginEngine;

impl LuaPluginEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn call_hook_json(
        &self,
        runtime_kind: &str,
        hook_name: &str,
        ctx: &Value,
    ) -> Result<String, String> {
        let lua = Lua::new();
        let globals = lua.globals();
        let panel = lua.create_table().map_err(|error| error.to_string())?;

        let log = lua
            .create_function(|_, msg: String| {
                println!("[Lua] {msg}");
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        panel.set("log", log).map_err(|error| error.to_string())?;

        let execute = lua
            .create_function(|lua_ctx, (cmd, args): (String, Vec<String>)| {
                let mut command = Command::new(&cmd);
                command.args(&args);
                #[cfg(windows)]
                command.creation_flags(CREATE_NO_WINDOW);

                match command.output() {
                    Ok(output) => {
                        let result = lua_ctx.create_table()?;
                        result.set(
                            "stdout",
                            String::from_utf8_lossy(&output.stdout).to_string(),
                        )?;
                        result.set(
                            "stderr",
                            String::from_utf8_lossy(&output.stderr).to_string(),
                        )?;
                        result.set("code", output.status.code().unwrap_or(-1))?;
                        Ok(result)
                    }
                    Err(error) => Err(mlua::Error::external(format!("Command failed: {error}"))),
                }
            })
            .map_err(|error| error.to_string())?;
        panel
            .set("execute", execute)
            .map_err(|error| error.to_string())?;

        let spawn = lua
            .create_function(|_, (cmd, args): (String, Vec<String>)| {
                let mut command = Command::new(&cmd);
                command.args(&args);
                command.stdin(Stdio::null());
                command.stdout(Stdio::null());
                command.stderr(Stdio::null());
                #[cfg(windows)]
                command.creation_flags(CREATE_NO_WINDOW);

                let mut child = command.spawn().map_err(mlua::Error::external)?;
                let pid = child.id();
                let started_at = Instant::now();
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            let code = status.code().unwrap_or(-1);
                            return Err(mlua::Error::external(format!(
                                "Process exited early with code {code}"
                            )));
                        }
                        Ok(None) => {
                            if started_at.elapsed() >= Duration::from_millis(1500) {
                                break;
                            }
                        }
                        Err(error) => return Err(mlua::Error::external(error)),
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Ok(pid)
            })
            .map_err(|error| error.to_string())?;
        panel
            .set("spawn", spawn)
            .map_err(|error| error.to_string())?;

        let spawn_detached = lua
            .create_function(|_, (cmd, args): (String, Vec<String>)| {
                let mut command = Command::new(&cmd);
                command.args(&args);
                command.stdin(Stdio::null());
                command.stdout(Stdio::null());
                command.stderr(Stdio::null());
                #[cfg(windows)]
                command.creation_flags(CREATE_NO_WINDOW);

                let mut child = command.spawn().map_err(mlua::Error::external)?;
                let pid = child.id();
                let started_at = Instant::now();
                while started_at.elapsed() < Duration::from_millis(1500) {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) => {}
                        Err(error) => return Err(mlua::Error::external(error)),
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Ok(pid)
            })
            .map_err(|error| error.to_string())?;
        panel
            .set("spawn_detached", spawn_detached)
            .map_err(|error| error.to_string())?;

        let write_file = lua
            .create_function(|_, (path, content): (String, String)| {
                let path = PathBuf::from(path);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(mlua::Error::external)?;
                }
                fs::write(path, content).map_err(mlua::Error::external)?;
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        panel
            .set("write_file", write_file)
            .map_err(|error| error.to_string())?;

        let read_file = lua
            .create_function(|_, path: String| {
                fs::read_to_string(path).map_err(mlua::Error::external)
            })
            .map_err(|error| error.to_string())?;
        panel
            .set("read_file", read_file)
            .map_err(|error| error.to_string())?;

        let exists = lua
            .create_function(|_, path: String| Ok(Path::new(&path).exists()))
            .map_err(|error| error.to_string())?;
        panel
            .set("exists", exists)
            .map_err(|error| error.to_string())?;

        let is_dir = lua
            .create_function(|_, path: String| Ok(Path::new(&path).is_dir()))
            .map_err(|error| error.to_string())?;
        panel
            .set("is_dir", is_dir)
            .map_err(|error| error.to_string())?;

        let to_unix_path = lua
            .create_function(|_, path: String| Ok(path.replace('\\', "/")))
            .map_err(|error| error.to_string())?;
        panel
            .set("to_unix_path", to_unix_path)
            .map_err(|error| error.to_string())?;

        let mkdir = lua
            .create_function(|_, path: String| {
                fs::create_dir_all(path).map_err(mlua::Error::external)
            })
            .map_err(|error| error.to_string())?;
        panel
            .set("mkdir", mkdir)
            .map_err(|error| error.to_string())?;

        let copy_file = lua
            .create_function(|_, (src, dest): (String, String)| {
                fs::copy(src, dest).map_err(mlua::Error::external)?;
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        panel
            .set("copy_file", copy_file)
            .map_err(|error| error.to_string())?;

        let remove_file = lua
            .create_function(|_, path: String| match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(mlua::Error::external(error)),
            })
            .map_err(|error| error.to_string())?;
        panel
            .set("remove_file", remove_file)
            .map_err(|error| error.to_string())?;

        let read_dir = lua
            .create_function(|lua_ctx, path: String| {
                let table = lua_ctx.create_table()?;
                let entries = fs::read_dir(path).map_err(mlua::Error::external)?;
                for (index, entry) in entries.enumerate() {
                    let entry = entry.map_err(mlua::Error::external)?;
                    table.set(index + 1, entry.path().display().to_string())?;
                }
                Ok(table)
            })
            .map_err(|error| error.to_string())?;
        panel
            .set("read_dir", read_dir)
            .map_err(|error| error.to_string())?;

        globals
            .set("panel", panel)
            .map_err(|error| error.to_string())?;

        let plugin_path = resolve_resource_base_dir()
            .ok_or_else(|| "Data dir not found".to_string())?
            .join("data")
            .join("plugins")
            .join(format!("{runtime_kind}.lua"));

        if !plugin_path.exists() {
            return Err(format!("Plugin not found: {}", plugin_path.display()));
        }

        let script = fs::read_to_string(&plugin_path).map_err(|error| error.to_string())?;
        let plugin = lua
            .load(&script)
            .eval::<mlua::Table>()
            .map_err(|error| error.to_string())?;

        let lua_ctx = json_to_lua_value(&lua, ctx).map_err(|error| error.to_string())?;
        let hook_res: mlua::Result<mlua::Function> = plugin.get(hook_name);
        if let Ok(hook) = hook_res {
            hook.call::<String>(lua_ctx)
                .map_err(|error: mlua::Error| error.to_string())
        } else {
            Err(format!(
                "Hook '{hook_name}' not found in plugin '{runtime_kind}'"
            ))
        }
    }

    pub fn call_hook(
        &self,
        runtime_kind: &str,
        hook_name: &str,
        ctx: HashMap<String, String>,
    ) -> Result<String, String> {
        let mut payload = Map::new();
        for (key, value) in ctx {
            payload.insert(key, Value::String(value));
        }
        self.call_hook_json(runtime_kind, hook_name, &Value::Object(payload))
    }
}

fn json_to_lua_value(lua: &Lua, value: &Value) -> mlua::Result<mlua::Value> {
    Ok(match value {
        Value::Null => mlua::Value::Nil,
        Value::Bool(value) => mlua::Value::Boolean(*value),
        Value::Number(value) => {
            if let Some(integer) = value.as_i64() {
                mlua::Value::Integer(integer)
            } else if let Some(float) = value.as_f64() {
                mlua::Value::Number(float)
            } else {
                mlua::Value::Nil
            }
        }
        Value::String(value) => mlua::Value::String(lua.create_string(value)?),
        Value::Array(values) => {
            let table = lua.create_table()?;
            for (index, item) in values.iter().enumerate() {
                table.set(index + 1, json_to_lua_value(lua, item)?)?;
            }
            mlua::Value::Table(table)
        }
        Value::Object(values) => {
            let table = lua.create_table()?;
            for (key, item) in values {
                table.set(key.as_str(), json_to_lua_value(lua, item)?)?;
            }
            mlua::Value::Table(table)
        }
    })
}

#[derive(Serialize, Clone)]
pub struct SoftwareTypeEntry {
    id: i64,
    title: String,
}

#[derive(Serialize, Clone)]
pub struct SoftwarePluginEntry {
    id: String,
    name: String,
    title: String,
    version: String,
    developer: String,
    description: String,
    price: f64,
    expire: String,
    category: String,
    installed: bool,
    status: String,
    path: String,
    actions: Vec<String>,
    visual: String,
}

#[derive(Clone)]
struct CachedSoftwareView {
    key: u64,
    software_types: Vec<SoftwareTypeEntry>,
    software_plugins: Vec<SoftwarePluginEntry>,
}

#[derive(Default)]
struct SoftwareRefreshState {
    in_progress: bool,
    last_check: Option<SystemTime>,
}

struct RuntimeInspection {
    system: System,
    #[cfg(windows)]
    listening_tcp_pids: HashMap<u16, Vec<u32>>,
}

impl RuntimeInspection {
    fn collect() -> Self {
        Self {
            system: collect_process_system(),
            #[cfg(windows)]
            listening_tcp_pids: collect_listening_tcp_pids(),
        }
    }
}

#[derive(Serialize, Clone)]
pub struct PhpRuntimeOption {
    id: String,
    label: String,
    version: String,
}

#[derive(Serialize, Clone)]
pub struct WebServerRuntime {
    pub(crate) kind: String,
    pub(crate) label: String,
    pub(crate) version: String,
    pub(crate) status: String,
}

#[derive(Deserialize)]
struct PluginStoreFile {
    #[serde(default)]
    r#type: Vec<PluginTypeRaw>,
    #[serde(default)]
    list: Vec<PluginRaw>,
}

#[derive(Deserialize)]
struct PluginTypeRaw {
    id: i64,
    title: String,
    #[serde(default)]
    sort: i64,
}

#[derive(Deserialize, Clone)]
struct PluginRaw {
    name: String,
    title: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    ps: String,
    #[serde(default)]
    price: serde_json::Value,
    #[serde(default)]
    endtime: i64,
    #[serde(default)]
    r#type: i64,
    #[serde(default)]
    sort: i64,
    #[serde(default)]
    dependent: String,
    #[serde(default)]
    install_checks: String,
    #[serde(default)]
    versions: Vec<PluginVersionRaw>,
}

#[derive(Deserialize, Clone, Default)]
struct PluginVersionRaw {
    #[serde(default)]
    version: String,
    #[serde(default)]
    full_version: String,
    #[serde(default)]
    f_path: String,
    #[serde(default)]
    install_checks: String,
    #[serde(default, alias = "download_urls", alias = "urls")]
    downloads: Vec<String>,
    #[serde(default)]
    mirrors: Vec<String>,
}

#[derive(Serialize)]
pub struct OperationStatus {
    pub(crate) status: bool,
    pub(crate) message: String,
}

#[derive(Deserialize, Default)]
pub struct DashboardDataQuery {
    #[serde(default)]
    software_sync: bool,
    #[serde(default)]
    view: Option<String>,
}

#[derive(Deserialize)]
pub struct SoftwareDownloadRequest {
    id: String,
}

#[derive(Deserialize)]
pub struct DatabaseCreateRequest {
    name: String,
    username: String,
    password: String,
}

#[derive(Deserialize)]
pub struct DatabaseRootPasswordRequest {
    password: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct RuntimeRegistry {
    #[serde(default)]
    pub(crate) entries: Vec<InstalledRuntime>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct InstalledRuntime {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) title: String,
    pub(crate) version: String,
    pub(crate) runtime_kind: String,
    pub(crate) install_dir: String,
    pub(crate) package_file: String,
    pub(crate) executable_path: Option<String>,
    pub(crate) state: String,
    pub(crate) pid: Option<u32>,
    pub(crate) php_port: Option<u16>,
}

#[derive(Clone)]
struct DownloadedPluginBundle {
    plugin_name: String,
    plugin_title: String,
    version: String,
    runtime_kind: String,
    package_file: PathBuf,
}

fn render_page(title: &str, topbar: &str, content: &str) -> Html<String> {
    let page = include_str!("ui/dashboard/layout.html")
        .replace("{{TITLE}}", title)
        .replace("{{TOPBAR}}", topbar)
        .replace("{{CONTENT}}", content);
    Html(page)
}

pub async fn page() -> Html<String> {
    render_page(
        "MinPanel Dashboard",
        include_str!("ui/dashboard/topbar.html"),
        include_str!("ui/dashboard/index.html"),
    )
}

pub async fn software_page() -> Html<String> {
    render_page(
        "MinPanel App Store",
        include_str!("ui/dashboard/topbar.html"),
        include_str!("ui/dashboard/soft.html"),
    )
}

pub async fn database_page() -> Html<String> {
    render_page(
        "MinPanel Database",
        include_str!("ui/dashboard/topbar.html"),
        include_str!("ui/dashboard/database.html"),
    )
}

pub async fn data(Query(query): Query<DashboardDataQuery>) -> Json<DashboardData> {
    let view = query.view.as_deref().unwrap_or("dashboard");
    let include_websites = matches!(view, "dashboard" | "website");
    let include_databases = matches!(view, "database");
    let include_software = matches!(view, "dashboard" | "website" | "software" | "database");
    let include_process_snapshot = matches!(view, "dashboard" | "processes");

    let mut system = if include_process_snapshot {
        System::new_all()
    } else {
        System::new()
    };
    system.refresh_cpu_usage();
    system.refresh_memory();
    if include_process_snapshot {
        system.refresh_processes_specifics(ProcessRefreshKind::everything());
    }

    let load_avg = System::load_average();
    let logical_cpu_count = system.cpus().len().max(1);
    let aa_panel_memory = read_aa_panel_memory_info()
        .unwrap_or_else(|| fallback_memory_info(system.total_memory(), system.used_memory()));
    let hostname = System::host_name().unwrap_or_else(|| "localhost".to_string());
    let primary_ip = detect_primary_ip().unwrap_or_else(|| "Unavailable".to_string());
    let workspace_root = env::current_dir()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| ".".to_string());
    let website_root = website::resolve_website_root().display().to_string();
    let (workspace_site_count, ftp_count, database_count) = summarize_workspace();
    let registry = load_runtime_registry().unwrap_or_default();
    let include_php_runtimes = matches!(view, "dashboard" | "website" | "database");
    let php_runtimes = if include_php_runtimes {
        registry_php_options(&registry)
    } else {
        Vec::new()
    };
    let web_server = if include_websites {
        active_web_server_runtime(&registry)
    } else {
        None
    };
    let websites = if include_websites {
        website::collect_websites(&registry)
    } else {
        Vec::new()
    };
    let (software_types, software_plugins) = if include_software {
        collect_software_store(query.software_sync, &registry).await
    } else {
        default_software_store()
    };
    let databases = if include_databases {
        collect_database_entries()
    } else {
        Vec::new()
    };
    let site_count = websites.len().max(workspace_site_count);
    let database_count = database_count.max(databases.len());
    let disks = collect_disks();
    let app_disk = find_app_disk(&disks, &workspace_root);
    let networks = collect_networks();
    let top_processes = if include_process_snapshot {
        collect_processes(&system)
    } else {
        Vec::new()
    };
    let alerts = build_alerts(
        system.global_cpu_info().cpu_usage(),
        aa_panel_memory.real_used,
        aa_panel_memory.total,
        &disks,
    );

    Json(DashboardData {
        hostname,
        primary_ip,
        os_name: System::name().unwrap_or_else(|| "Unknown OS".to_string()),
        kernel_version: System::kernel_version().unwrap_or_else(|| "Unknown".to_string()),
        uptime: System::uptime(),
        cpu_usage: system.global_cpu_info().cpu_usage(),
        cpu_brand: system
            .cpus()
            .first()
            .map(|cpu| cpu.brand().to_string())
            .unwrap_or_else(|| "Generic CPU".to_string()),
        cpu_frequency: system
            .cpus()
            .first()
            .map(|cpu| cpu.frequency())
            .unwrap_or_default(),
        cpu_cores: logical_cpu_count,
        total_memory: aa_panel_memory.total,
        used_memory: system.used_memory(),
        real_used_memory: aa_panel_memory.real_used,
        free_memory: aa_panel_memory.free,
        buffered_memory: aa_panel_memory.buffers,
        cached_memory: aa_panel_memory.cached,
        total_swap: system.total_swap(),
        used_swap: system.used_swap(),
        app_disk,
        load_avg: LoadAverageData {
            one: load_avg.one,
            five: load_avg.five,
            fifteen: load_avg.fifteen,
            max: (logical_cpu_count * 2) as f64,
            safe: (logical_cpu_count as f64) * 1.5,
        },
        process_count: if include_process_snapshot {
            system.processes().len()
        } else {
            0
        },
        site_count,
        ftp_count,
        database_count,
        warning_count: alerts.len(),
        websites,
        php_runtimes,
        web_server,
        software_types,
        software_plugins,
        workspace_root,
        website_root,
        databases,
        disks,
        networks,
        top_processes,
        alerts,
    })
}

pub async fn refresh_software_store() -> Json<OperationStatus> {
    match sync_software_store(true).await {
        Ok(_) => Json(OperationStatus {
            status: true,
            message: "Software list updated!".to_string(),
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn download_software_package(
    Json(request): Json<SoftwareDownloadRequest>,
) -> Json<OperationStatus> {
    match download_plugin_package(&request.id).await {
        Ok(path) => Json(OperationStatus {
            status: true,
            message: format!("Plugin package downloaded to {}", path.display()),
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn install_software_package(
    Json(request): Json<SoftwareDownloadRequest>,
) -> Json<OperationStatus> {
    let task_id = uuid::Uuid::new_v4().to_string();
    let plugin_id = request.id.clone();

    // Create task entry
    {
        if let Ok(mut tasks) = get_task_manager().lock() {
            tasks.insert(
                task_id.clone(),
                TaskInfo {
                    id: task_id.clone(),
                    name: format!("Install[{}]", plugin_id),
                    status: "running".to_string(),
                    log: "".to_string(),
                    last_message: "Starting...".to_string(),
                    created_at: SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                },
            );
        }
    }

    // Spawn background task
    let tid = task_id.clone();
    tokio::spawn(async move {
        match install_plugin_package(&plugin_id, &tid).await {
            Ok(message) => {
                update_task_log(&tid, &format!("Ready: {message}"));
                set_task_status(&tid, "success");
            }
            Err(error) => {
                update_task_log(&tid, &format!("Error: {error}"));
                set_task_status(&tid, "failed");
            }
        }
    });

    Json(OperationStatus {
        status: true,
        message: task_id,
    })
}

pub async fn list_tasks() -> Json<Vec<TaskInfo>> {
    if let Ok(tasks) = get_task_manager().lock() {
        let mut list: Vec<TaskInfo> = tasks.values().cloned().collect();
        list.sort_by_key(|t| std::cmp::Reverse(t.created_at));
        Json(list)
    } else {
        Json(Vec::new())
    }
}

pub async fn get_task_log(axum::extract::Path(id): axum::extract::Path<String>) -> Json<Value> {
    if let Ok(tasks) = get_task_manager().lock() {
        if let Some(task) = tasks.get(&id) {
            return Json(json!({ "id": id, "log": task.log, "status": task.status }));
        }
    }
    Json(json!({ "status": "failed", "message": "Task not found" }))
}

pub async fn start_software_package(
    Json(request): Json<SoftwareDownloadRequest>,
) -> Json<OperationStatus> {
    match start_installed_runtime(&request.id) {
        Ok(message) => Json(OperationStatus {
            status: true,
            message,
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn start_all_software_packages() -> Json<OperationStatus> {
    match start_all_installed_runtimes() {
        Ok(message) => Json(OperationStatus {
            status: true,
            message,
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn stop_all_software_packages() -> Json<OperationStatus> {
    match stop_all_installed_runtimes() {
        Ok(message) => Json(OperationStatus {
            status: true,
            message,
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn stop_software_package(
    Json(request): Json<SoftwareDownloadRequest>,
) -> Json<OperationStatus> {
    println!("[Software] STOP requested for id: {}", request.id);
    match stop_installed_runtime(&request.id) {
        Ok(message) => {
            println!("[Software] STOP success: {}", message);
            Json(OperationStatus {
                status: true,
                message,
            })
        }
        Err(error) => {
            println!("[Software] STOP failed: {}", error);
            Json(OperationStatus {
                status: false,
                message: error,
            })
        }
    }
}

pub async fn restart_software_package(
    Json(request): Json<SoftwareDownloadRequest>,
) -> Json<OperationStatus> {
    match restart_installed_runtime(&request.id) {
        Ok(message) => Json(OperationStatus {
            status: true,
            message,
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn reload_software_package(
    Json(request): Json<SoftwareDownloadRequest>,
) -> Json<OperationStatus> {
    match reload_installed_runtime(&request.id) {
        Ok(message) => Json(OperationStatus {
            status: true,
            message,
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn uninstall_software_package(
    Json(request): Json<SoftwareDownloadRequest>,
) -> Json<OperationStatus> {
    match uninstall_installed_runtime(&request.id) {
        Ok(message) => Json(OperationStatus {
            status: true,
            message,
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn open_software_install_path(
    Json(request): Json<SoftwareDownloadRequest>,
) -> Json<OperationStatus> {
    match open_installed_runtime_path(&request.id) {
        Ok(message) => Json(OperationStatus {
            status: true,
            message,
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn create_database(Json(request): Json<DatabaseCreateRequest>) -> Json<OperationStatus> {
    match create_mysql_database(&request) {
        Ok(message) => Json(OperationStatus {
            status: true,
            message,
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn set_database_root_password(
    Json(request): Json<DatabaseRootPasswordRequest>,
) -> Json<OperationStatus> {
    match set_mysql_root_password(&request.password) {
        Ok(message) => Json(OperationStatus {
            status: true,
            message,
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn phpmyadmin_proxy(
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match proxy_phpmyadmin_request(
        method,
        uri.path_and_query().map(|value| value.as_str()),
        headers,
        body,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            error,
        )
            .into_response(),
    }
}

struct AaPanelMemoryInfo {
    total: u64,
    free: u64,
    buffers: u64,
    cached: u64,
    real_used: u64,
}

fn read_aa_panel_memory_info() -> Option<AaPanelMemoryInfo> {
    let contents = fs::read_to_string("/proc/meminfo").ok()?;
    let mut values = HashMap::new();

    for line in contents.lines() {
        let mut parts = line.split(':');
        let key = parts.next()?.trim();
        let value_part = parts.next()?.trim();
        let kb = value_part
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u64>().ok())?;
        values.insert(key.to_string(), kb * 1024);
    }

    let total = *values.get("MemTotal")?;
    let free = values.get("MemFree").copied().unwrap_or_default();
    let buffers = values.get("Buffers").copied().unwrap_or_default();
    let cached = values.get("Cached").copied().unwrap_or_default();
    let real_used = total.saturating_sub(free.saturating_add(buffers).saturating_add(cached));

    Some(AaPanelMemoryInfo {
        total,
        free,
        buffers,
        cached,
        real_used,
    })
}

fn fallback_memory_info(total: u64, used: u64) -> AaPanelMemoryInfo {
    AaPanelMemoryInfo {
        total,
        free: total.saturating_sub(used),
        buffers: 0,
        cached: 0,
        real_used: used,
    }
}

fn detect_primary_ip() -> Option<String> {
    let candidates = [
        SocketAddr::from(([8, 8, 8, 8], 80)),
        SocketAddr::from(([1, 1, 1, 1], 80)),
        SocketAddr::from(([208, 67, 222, 222], 80)),
    ];

    for target in candidates {
        let socket = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
        if socket.connect(target).is_err() {
            continue;
        }

        let ip = socket.local_addr().ok()?.ip();
        if is_usable_ip(ip) {
            return Some(ip.to_string());
        }
    }

    None
}

fn is_usable_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => !ipv4.is_loopback() && !ipv4.is_unspecified(),
        IpAddr::V6(ipv6) => !ipv6.is_loopback() && !ipv6.is_unspecified(),
    }
}

fn summarize_workspace() -> (usize, usize, usize) {
    let mut directory_count = 0;
    let mut file_count = 0;
    let mut database_count = 0;
    let current_dir = match env::current_dir() {
        Ok(path) => path,
        Err(_) => return (0, 0, 0),
    };

    let entries = match fs::read_dir(current_dir) {
        Ok(entries) => entries,
        Err(_) => return (0, 0, 0),
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" {
            continue;
        }

        if let Ok(file_type) = entry.file_type() {
            if file_type.is_dir() {
                directory_count += 1;
            } else if file_type.is_file() {
                file_count += 1;
                if is_database_file(&name) {
                    database_count += 1;
                }
            }
        }
    }

    (directory_count, file_count, database_count)
}

fn is_database_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".db")
        || lower.ends_with(".sqlite")
        || lower.ends_with(".sqlite3")
        || lower.ends_with(".sql")
}

fn collect_database_entries() -> Vec<DatabaseEntry> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let credentials = load_database_credentials();

    collect_mysql_database_entries(&mut entries, &mut seen, &credentials);

    if let Ok(current_dir) = env::current_dir() {
        collect_database_entries_from_dir(&current_dir, "Workspace", 0, &mut seen, &mut entries);
    }

    let website_root = website::resolve_website_root();
    collect_database_entries_from_dir(&website_root, "Website root", 0, &mut seen, &mut entries);

    entries.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    entries.truncate(200);
    entries
}

fn collect_database_entries_from_dir(
    root: &Path,
    source: &str,
    depth: usize,
    seen: &mut HashSet<String>,
    entries: &mut Vec<DatabaseEntry>,
) {
    if depth > 4 || entries.len() >= 200 {
        return;
    }

    let read_dir = match fs::read_dir(root) {
        Ok(read_dir) => read_dir,
        Err(_) => return,
    };

    for entry in read_dir.flatten() {
        if entries.len() >= 200 {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if should_skip_database_scan_entry(&name) {
            continue;
        }

        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };

        if file_type.is_dir() {
            collect_database_entries_from_dir(&path, source, depth + 1, seen, entries);
            continue;
        }

        if !file_type.is_file() || !is_database_file(&name) {
            continue;
        }

        let canonical_key = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .display()
            .to_string()
            .to_ascii_lowercase();
        if !seen.insert(canonical_key) {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };

        let modified_ms = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let display_path = path.display().to_string();
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .filter(|stem| !stem.trim().is_empty())
            .unwrap_or_else(|| name.clone());

        entries.push(DatabaseEntry {
            id: slugify(&display_path, '-'),
            name: stem,
            engine: database_engine_from_file_name(&name),
            username: "local file".to_string(),
            password: "--".to_string(),
            permission: "Local".to_string(),
            backup_count: 0,
            size: metadata.len(),
            modified_ms,
            path: display_path,
            source: source.to_string(),
        });
    }
}

fn should_skip_database_scan_entry(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        ".git" | "target" | "aapanel" | "node_modules" | ".gocache"
    )
}

fn database_engine_from_file_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".sql") {
        "SQL".to_string()
    } else {
        "SQLite".to_string()
    }
}

fn create_mysql_database(request: &DatabaseCreateRequest) -> Result<String, String> {
    let database_name = validate_mysql_identifier(&request.name, "Database name")?;
    let username = validate_mysql_identifier(&request.username, "Username")?;
    let password = request.password.trim();
    if password.is_empty() {
        return Err("Password is required".to_string());
    }
    if password.chars().count() > 128 {
        return Err("Password must be 128 characters or less".to_string());
    }

    let registry = load_runtime_registry()?;
    let mysql = select_primary_runtime(&registry, "mysql")
        .ok_or_else(|| "MySQL is not installed. Install MySQL from App Store first.".to_string())?;
    let client_path = mysql_client_path(mysql)
        .ok_or_else(|| "mysql.exe was not found in the MySQL install directory.".to_string())?;
    if !mysql_runtime_is_available(mysql) {
        return Err("MySQL is not running. Start MySQL before adding a database.".to_string());
    }

    let escaped_password = escape_mysql_string(password);
    let sql = format!(
        "CREATE DATABASE IF NOT EXISTS `{database_name}` DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;\
         CREATE USER IF NOT EXISTS '{username}'@'localhost' IDENTIFIED BY '{escaped_password}';\
         CREATE USER IF NOT EXISTS '{username}'@'127.0.0.1' IDENTIFIED BY '{escaped_password}';\
         GRANT ALL PRIVILEGES ON `{database_name}`.* TO '{username}'@'localhost';\
         GRANT ALL PRIVILEGES ON `{database_name}`.* TO '{username}'@'127.0.0.1';\
         FLUSH PRIVILEGES;"
    );

    run_mysql_command(&client_path, &sql)?;
    if let Err(error) = save_mysql_database_credential(&database_name, &username, password) {
        eprintln!(
            "[Database] Failed to save credential for {}: {}",
            database_name, error
        );
    }
    Ok(format!("Database {database_name} created"))
}

fn set_mysql_root_password(password: &str) -> Result<String, String> {
    let password = password.trim();
    if password.is_empty() {
        return Err("Password is required".to_string());
    }
    if password.chars().count() > 128 {
        return Err("Password must be 128 characters or less".to_string());
    }

    let registry = load_runtime_registry()?;
    let mysql = select_primary_runtime(&registry, "mysql")
        .ok_or_else(|| "MySQL is not installed. Install MySQL from App Store first.".to_string())?;
    let client_path = mysql_client_path(mysql)
        .ok_or_else(|| "mysql.exe was not found in the MySQL install directory.".to_string())?;
    if !mysql_runtime_is_available(mysql) {
        return Err("MySQL is not running. Start MySQL before changing root password.".to_string());
    }

    let escaped_password = escape_mysql_string(password);
    // On Windows, the mysql client might be configured to use empty password initially.
    // If it's already set, we might need a more complex way to change it,
    // but usually aaPanel-style setups allow this if we are running as administrator/local system.
    let sql = format!(
        "ALTER USER 'root'@'localhost' IDENTIFIED BY '{escaped_password}'; FLUSH PRIVILEGES;"
    );

    run_mysql_command(&client_path, &sql)?;
    Ok("Root password changed successfully".to_string())
}

fn load_database_credentials() -> DatabaseCredentialStore {
    let Ok(path) = database_credentials_path() else {
        return DatabaseCredentialStore::default();
    };
    let Ok(contents) = fs::read_to_string(path) else {
        return DatabaseCredentialStore::default();
    };
    serde_json::from_str::<DatabaseCredentialStore>(&contents).unwrap_or_default()
}

fn save_database_credentials(store: &DatabaseCredentialStore) -> Result<(), String> {
    let path = database_credentials_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create database credential directory: {error}"))?;
    }
    let contents = serde_json::to_string_pretty(store)
        .map_err(|error| format!("Failed to serialize database credentials: {error}"))?;
    fs::write(&path, contents)
        .map_err(|error| format!("Failed to write database credentials: {error}"))?;
    Ok(())
}

fn save_mysql_database_credential(
    name: &str,
    username: &str,
    password: &str,
) -> Result<(), String> {
    let mut store = load_database_credentials();
    store.mysql.insert(
        name.to_ascii_lowercase(),
        StoredDatabaseCredential {
            username: username.to_string(),
            password: password.to_string(),
        },
    );
    save_database_credentials(&store)
}

fn validate_mysql_identifier(value: &str, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label} is required"));
    }
    if value.len() > 64 {
        return Err(format!("{label} must be 64 characters or less"));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(format!(
            "{label} can only contain letters, numbers, and underscores"
        ));
    }
    Ok(value.to_string())
}

fn escape_mysql_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn mysql_client_path(entry: &InstalledRuntime) -> Option<PathBuf> {
    let install_dir = Path::new(&entry.install_dir);
    let candidates = [
        install_dir
            .join("bin")
            .join(if cfg!(windows) { "mysql.exe" } else { "mysql" }),
        install_dir.join(if cfg!(windows) { "mysql.exe" } else { "mysql" }),
    ];
    candidates.into_iter().find(|path| path.exists())
}

fn mysql_runtime_is_available(entry: &InstalledRuntime) -> bool {
    let inspection = RuntimeInspection::collect();
    is_runtime_available_after_start_with_inspection(entry, &inspection)
}

fn run_mysql_command(client_path: &Path, sql: &str) -> Result<String, String> {
    let mut command = Command::new(client_path);
    let output = hide_windows_console_window(
        command
            .args([
                "--protocol=TCP",
                "-h",
                "127.0.0.1",
                "-P",
                "3306",
                "-uroot",
                "--default-character-set=utf8mb4",
                "-e",
                sql,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .output()
    .map_err(|error| format!("Failed to run mysql client: {error}"))?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(if detail.is_empty() {
        "MySQL command failed".to_string()
    } else {
        detail
    })
}

fn collect_mysql_database_entries(
    entries: &mut Vec<DatabaseEntry>,
    seen: &mut HashSet<String>,
    credentials: &DatabaseCredentialStore,
) {
    let Ok(registry) = load_runtime_registry() else {
        return;
    };
    let Some(mysql) = select_primary_runtime(&registry, "mysql") else {
        return;
    };
    if !mysql_runtime_is_available(mysql) {
        return;
    }
    let Some(client_path) = mysql_client_path(mysql) else {
        return;
    };

    let sql = "SELECT s.schema_name, COALESCE(SUM(t.data_length + t.index_length), 0) \
               FROM information_schema.schemata s \
               LEFT JOIN information_schema.tables t ON s.schema_name = t.table_schema \
               WHERE s.schema_name NOT IN ('information_schema','mysql','performance_schema','sys') \
               GROUP BY s.schema_name \
               ORDER BY s.schema_name;";
    let output = match run_mysql_command_with_args(&client_path, &["-N", "-B", "-e", sql]) {
        Ok(output) => output,
        Err(_) => return,
    };

    for line in output.lines() {
        if entries.len() >= 200 {
            return;
        }
        let mut parts = line.split('\t');
        let Some(name) = parts.next().map(str::trim).filter(|name| !name.is_empty()) else {
            continue;
        };
        let size = parts
            .next()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or_default();
        let key = format!("mysql:{name}").to_ascii_lowercase();
        if !seen.insert(key.clone()) {
            continue;
        }
        let stored = credentials.mysql.get(&name.to_ascii_lowercase());
        entries.push(DatabaseEntry {
            id: slugify(&key, '-'),
            name: name.to_string(),
            engine: "MySQL".to_string(),
            username: stored
                .map(|credential| credential.username.clone())
                .unwrap_or_else(|| name.to_string()),
            password: stored
                .map(|credential| credential.password.clone())
                .unwrap_or_default(),
            permission: "Local".to_string(),
            backup_count: 0,
            size,
            modified_ms: 0,
            path: format!("127.0.0.1:3306/{name}"),
            source: "MySQL".to_string(),
        });
    }
}

fn run_mysql_command_with_args(client_path: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(client_path);
    let output = hide_windows_console_window(
        command
            .args([
                "--protocol=TCP",
                "-h",
                "127.0.0.1",
                "-P",
                "3306",
                "-uroot",
                "--default-character-set=utf8mb4",
            ])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .output()
    .map_err(|error| format!("Failed to run mysql client: {error}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if detail.is_empty() {
            "MySQL command failed".to_string()
        } else {
            detail
        })
    }
}

async fn proxy_phpmyadmin_request(
    method: Method,
    path_and_query: Option<&str>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, String> {
    let registry = load_runtime_registry()?;
    let phpmyadmin = select_primary_runtime(&registry, "phpmyadmin").ok_or_else(|| {
        "phpMyAdmin is not installed. Install it from App Store first.".to_string()
    })?;
    let phpmyadmin_root = PathBuf::from(&phpmyadmin.install_dir);
    if !phpmyadmin_root.join("index.php").exists() {
        return Err("phpMyAdmin index.php was not found in the install directory.".to_string());
    }
    let php = select_primary_runtime(&registry, "php").ok_or_else(|| {
        "PHP is not installed. Install PHP before opening phpMyAdmin.".to_string()
    })?;
    let php_cgi = php_cgi_path(php)
        .ok_or_else(|| "php-cgi.exe was not found in the PHP install directory.".to_string())?;
    ensure_phpmyadmin_php_extensions(php)?;
    ensure_phpmyadmin_config(&phpmyadmin_root)?;
    let request_target = path_and_query.unwrap_or("/phpmyadmin/");
    let (request_path, query_string) = split_request_target(request_target);
    let script_path = resolve_phpmyadmin_request_path(&phpmyadmin_root, request_path)?;

    if !script_path.exists() {
        return Ok((
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "phpMyAdmin resource not found",
        )
            .into_response());
    }

    if script_path.is_file() && !is_php_file(&script_path) {
        return serve_phpmyadmin_static_file(&script_path);
    }

    execute_phpmyadmin_script(
        &php_cgi,
        &phpmyadmin_root,
        &script_path,
        method,
        request_target,
        query_string,
        headers,
        body,
        php,
    )
}

fn split_request_target(request_target: &str) -> (&str, &str) {
    let mut parts = request_target.splitn(2, '?');
    let path = parts.next().unwrap_or("/phpmyadmin/");
    let query = parts.next().unwrap_or("");
    (path, query)
}

fn resolve_phpmyadmin_request_path(root: &Path, request_path: &str) -> Result<PathBuf, String> {
    let relative = request_path
        .strip_prefix("/phpmyadmin")
        .unwrap_or(request_path)
        .trim_start_matches('/');
    let relative = if relative.is_empty() {
        "index.php".to_string()
    } else {
        percent_decode_path(relative)?
    };

    let mut target = root.to_path_buf();
    for component in Path::new(&relative).components() {
        match component {
            std::path::Component::Normal(part) => target.push(part),
            std::path::Component::CurDir => {}
            _ => return Err("Invalid phpMyAdmin path".to_string()),
        }
    }

    if target.is_dir() {
        target.push("index.php");
    }
    Ok(target)
}

fn percent_decode_path(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err("Invalid encoded phpMyAdmin path".to_string());
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .map_err(|_| "Invalid encoded phpMyAdmin path".to_string())?;
            let byte = u8::from_str_radix(hex, 16)
                .map_err(|_| "Invalid encoded phpMyAdmin path".to_string())?;
            output.push(byte);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).map_err(|_| "Invalid UTF-8 phpMyAdmin path".to_string())
}

fn is_php_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("php"))
        .unwrap_or(false)
}

fn php_cgi_path(entry: &InstalledRuntime) -> Option<PathBuf> {
    let install_dir = Path::new(&entry.install_dir);
    [
        install_dir.join(if cfg!(windows) {
            "php-cgi.exe"
        } else {
            "php-cgi"
        }),
        install_dir.join("bin").join(if cfg!(windows) {
            "php-cgi.exe"
        } else {
            "php-cgi"
        }),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn ensure_phpmyadmin_php_extensions(entry: &InstalledRuntime) -> Result<(), String> {
    let install_dir = Path::new(&entry.install_dir);
    let php_ini = install_dir.join("php.ini");
    if !php_ini.exists() {
        let template = install_dir.join("php.ini-production");
        if template.exists() {
            fs::copy(&template, &php_ini)
                .map_err(|error| format!("Failed to initialize php.ini: {error}"))?;
        } else {
            fs::write(&php_ini, "")
                .map_err(|error| format!("Failed to create php.ini: {error}"))?;
        }
    }

    let contents =
        fs::read_to_string(&php_ini).map_err(|error| format!("Failed to read php.ini: {error}"))?;
    let normalized = normalize_php_ini_for_phpmyadmin(&contents, install_dir);
    if normalized != contents {
        fs::write(&php_ini, normalized)
            .map_err(|error| format!("Failed to update php.ini for phpMyAdmin: {error}"))?;
    }
    Ok(())
}

fn normalize_php_ini_for_phpmyadmin(contents: &str, install_dir: &Path) -> String {
    let extension_dir = install_dir
        .join("ext")
        .display()
        .to_string()
        .replace('\\', "/");
    let wanted = [
        (
            "extension_dir",
            format!("extension_dir = \"{extension_dir}\""),
        ),
        ("mysqli", "extension=mysqli".to_string()),
        ("mbstring", "extension=mbstring".to_string()),
        ("openssl", "extension=openssl".to_string()),
    ];
    let mut seen = HashSet::new();
    let mut lines = Vec::new();

    for line in contents.replace("\r\n", "\n").replace('\r', "\n").lines() {
        let trimmed = line.trim();
        let uncommented = trimmed.trim_start_matches(';').trim();
        let lower = uncommented.to_ascii_lowercase();
        let key = if lower.starts_with("extension_dir") && lower.contains('=') {
            Some("extension_dir")
        } else if lower == "extension=mysqli" || lower == "extension=php_mysqli.dll" {
            Some("mysqli")
        } else if lower == "extension=mbstring" || lower == "extension=php_mbstring.dll" {
            Some("mbstring")
        } else if lower == "extension=openssl" || lower == "extension=php_openssl.dll" {
            Some("openssl")
        } else {
            None
        };

        if let Some(key) = key {
            if seen.insert(key.to_string()) {
                if let Some((_, value)) = wanted.iter().find(|(wanted_key, _)| *wanted_key == key) {
                    lines.push(value.clone());
                }
            }
        } else {
            lines.push(line.to_string());
        }
    }

    for (key, value) in wanted {
        if !seen.contains(key) {
            lines.push(value);
        }
    }

    let mut output = lines.join("\r\n");
    output.push_str("\r\n");
    output
}

fn ensure_phpmyadmin_config(root: &Path) -> Result<(), String> {
    for name in ["tmp", "upload", "save"] {
        fs::create_dir_all(root.join(name))
            .map_err(|error| format!("Failed to create phpMyAdmin {name} directory: {error}"))?;
    }

    let config_path = root.join("config.inc.php");
    let desired = phpmyadmin_config_body(root);
    let current = fs::read_to_string(&config_path).unwrap_or_default();
    if current != desired || current.contains("/www/server/") {
        fs::write(&config_path, desired)
            .map_err(|error| format!("Failed to write phpMyAdmin config.inc.php: {error}"))?;
    }
    Ok(())
}

fn phpmyadmin_config_body(root: &Path) -> String {
    let seed = root.display().to_string();
    let hash = fast_hash(&seed);
    let secret = format!("MinPanel{:016x}PMAKey!!", hash);
    format!(
        "<?php\r\n\
/**\r\n\
 * MinPanel managed phpMyAdmin configuration.\r\n\
 */\r\n\
$cfg['blowfish_secret'] = '{}';\r\n\
$cfg['TempDir'] = __DIR__ . '/tmp';\r\n\
$cfg['UploadDir'] = __DIR__ . '/upload';\r\n\
$cfg['SaveDir'] = __DIR__ . '/save';\r\n\
\r\n\
$i = 0;\r\n\
$i++;\r\n\
$cfg['Servers'][$i]['auth_type'] = 'cookie';\r\n\
$cfg['Servers'][$i]['host'] = '127.0.0.1';\r\n\
$cfg['Servers'][$i]['port'] = '3306';\r\n\
$cfg['Servers'][$i]['compress'] = false;\r\n\
$cfg['Servers'][$i]['AllowNoPassword'] = true;\r\n\
\r\n\
$cfg['PmaNoRelation_DisableWarning'] = true;\r\n",
        php_single_quote_escape(&secret)
    )
}

fn php_single_quote_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

fn serve_phpmyadmin_static_file(path: &Path) -> Result<Response, String> {
    let body =
        fs::read(path).map_err(|error| format!("Failed to read phpMyAdmin file: {error}"))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, phpmyadmin_mime_type(path))
        .body(axum::body::Body::from(body))
        .map_err(|error| format!("Failed to build phpMyAdmin file response: {error}"))
}

fn phpmyadmin_mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "html" | "htm" => "text/html; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn execute_phpmyadmin_script(
    php_cgi: &Path,
    document_root: &Path,
    script_path: &Path,
    method: Method,
    request_uri: &str,
    query_string: &str,
    headers: HeaderMap,
    body: Bytes,
    php_entry: &InstalledRuntime,
) -> Result<Response, String> {
    let script_name = format!(
        "/phpmyadmin/{}",
        script_path
            .strip_prefix(document_root)
            .unwrap_or(script_path)
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    let mut command = Command::new(php_cgi);
    command
        .current_dir(document_root)
        .env("GATEWAY_INTERFACE", "CGI/1.1")
        .env("REDIRECT_STATUS", "200")
        .env("REQUEST_METHOD", method.as_str())
        .env("REQUEST_URI", request_uri)
        .env("QUERY_STRING", query_string)
        .env("SCRIPT_FILENAME", script_path)
        .env("SCRIPT_NAME", &script_name)
        .env("PHP_SELF", &script_name)
        .env("DOCUMENT_ROOT", document_root)
        .env("SERVER_SOFTWARE", "MinPanel")
        .env("SERVER_PROTOCOL", "HTTP/1.1")
        .env("SERVER_NAME", "localhost")
        .env("SERVER_PORT", preferred_panel_port_string())
        .env("CONTENT_LENGTH", body.len().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        command.env("CONTENT_TYPE", content_type);
    }
    if let Some(cookie) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
    {
        command.env("HTTP_COOKIE", cookie);
    }
    if let Some(user_agent) = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
    {
        command.env("HTTP_USER_AGENT", user_agent);
    }
    if let Some(accept) = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
    {
        command.env("HTTP_ACCEPT", accept);
    }
    if let Some(language) = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
    {
        command.env("HTTP_ACCEPT_LANGUAGE", language);
    }
    if let Some(referer) = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
    {
        command.env("HTTP_REFERER", referer);
    }
    let php_ini_dir = Path::new(&php_entry.install_dir);
    if php_ini_dir.join("php.ini").exists() {
        command.env("PHPRC", php_ini_dir);
    }

    let mut child = hide_windows_console_window(&mut command)
        .spawn()
        .map_err(|error| format!("Failed to start php-cgi: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&body)
            .map_err(|error| format!("Failed to write phpMyAdmin request body: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Failed to read php-cgi output: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!("php-cgi exited with status {}", output.status)
        } else {
            detail
        });
    }
    build_cgi_response(&output.stdout)
}

fn preferred_panel_port_string() -> String {
    env::var("MINI_PANEL_PORT").unwrap_or_else(|_| "8080".to_string())
}

fn build_cgi_response(output: &[u8]) -> Result<Response, String> {
    let (raw_headers, body) = split_cgi_output(output);
    let header_text = String::from_utf8_lossy(raw_headers);
    let mut status = StatusCode::OK;
    let mut builder = Response::builder();
    let mut has_content_type = false;
    for line in header_text.lines() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("Status") {
            if let Some(code) = value
                .split_whitespace()
                .next()
                .and_then(|code| code.parse::<u16>().ok())
            {
                status = StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
            }
            continue;
        }
        if name.eq_ignore_ascii_case("Content-Type") {
            has_content_type = true;
        }
        if !name.eq_ignore_ascii_case("X-Powered-By") {
            builder = builder.header(name, value);
        }
    }
    if !has_content_type {
        builder = builder.header(header::CONTENT_TYPE, "text/html; charset=utf-8");
    }
    builder
        .status(status)
        .body(axum::body::Body::from(body.to_vec()))
        .map_err(|error| format!("Failed to build phpMyAdmin CGI response: {error}"))
}

fn split_cgi_output(output: &[u8]) -> (&[u8], &[u8]) {
    if let Some(index) = output.windows(4).position(|window| window == b"\r\n\r\n") {
        return (&output[..index], &output[index + 4..]);
    }
    if let Some(index) = output.windows(2).position(|window| window == b"\n\n") {
        return (&output[..index], &output[index + 2..]);
    }
    (&[], output)
}

async fn collect_software_store(
    force_refresh: bool,
    registry: &RuntimeRegistry,
) -> (Vec<SoftwareTypeEntry>, Vec<SoftwarePluginEntry>) {
    let contents = match sync_software_store(force_refresh).await {
        Ok(contents) => contents,
        Err(_) => return default_software_store(),
    };
    let base_dir = match resolve_resource_base_dir() {
        Some(base_dir) => base_dir,
        None => return default_software_store(),
    };
    let cache_key = software_store_cache_key(&contents, registry);
    if let Some(cached) = software_view_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|cached| cached.key == cache_key)
        .cloned()
    {
        return (cached.software_types, cached.software_plugins);
    }

    let store = match serde_json::from_str::<PluginStoreFile>(&contents) {
        Ok(store) => store,
        Err(_) => return default_software_store(),
    };
    let (software_types, software_plugins) = map_plugin_store(store, &base_dir, registry);
    *software_view_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(CachedSoftwareView {
        key: cache_key,
        software_types: software_types.clone(),
        software_plugins: software_plugins.clone(),
    });
    (software_types, software_plugins)
}

fn software_view_cache() -> &'static Mutex<Option<CachedSoftwareView>> {
    SOFTWARE_VIEW_CACHE.get_or_init(|| Mutex::new(None))
}

fn software_refresh_state() -> &'static Mutex<SoftwareRefreshState> {
    SOFTWARE_REFRESH_STATE.get_or_init(|| Mutex::new(SoftwareRefreshState::default()))
}

fn software_store_cache_key(contents: &str, registry: &RuntimeRegistry) -> u64 {
    fast_hash(&(contents, runtime_registry_cache_signature(registry)))
}

fn runtime_registry_cache_signature(registry: &RuntimeRegistry) -> Vec<String> {
    let mut signature = registry
        .entries
        .iter()
        .filter(|entry| is_runtime_entry_ready(entry))
        .map(|entry| {
            format!(
                "{}|{}|{}|{}|{}|{}",
                entry.id,
                entry.name,
                entry.version,
                entry.runtime_kind,
                entry.state,
                entry.pid.unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    signature.sort();
    signature
}

fn fast_hash<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn select_plugin_version_entry(version: &PluginVersionRaw, fallback: &str) -> String {
    if !version.version.is_empty() && version.version != "0" {
        return version.version.clone();
    }
    if !version.full_version.is_empty() && version.full_version != "0" {
        return version.full_version.clone();
    }
    if !fallback.is_empty() && fallback != "0" {
        return fallback.to_string();
    }
    "--".to_string()
}

fn merge_plugin_version_defaults(
    plugin: &PluginRaw,
    version: &PluginVersionRaw,
) -> PluginVersionRaw {
    let mut merged = version.clone();
    if merged.install_checks.trim().is_empty() {
        merged.install_checks = plugin.install_checks.clone();
    }
    if merged.version.trim().is_empty() || merged.version == "0" {
        merged.version = plugin.version.clone();
    }
    if merged.full_version.trim().is_empty() || merged.full_version == "0" {
        merged.full_version = select_plugin_version_entry(&merged, &plugin.version);
    }
    merged
}

fn effective_plugin_versions(plugin: &PluginRaw) -> Vec<PluginVersionRaw> {
    if plugin.versions.is_empty() {
        return Vec::new();
    }
    plugin
        .versions
        .iter()
        .map(|entry| merge_plugin_version_defaults(plugin, entry))
        .collect()
}

fn expand_plugin_variants(plugin: PluginRaw) -> Vec<PluginRaw> {
    let versions = effective_plugin_versions(&plugin);
    if versions.is_empty() {
        return vec![plugin];
    }

    versions
        .into_iter()
        .map(|version_entry| {
            let mut variant = plugin.clone();
            variant.version = select_plugin_version_entry(&version_entry, &plugin.version);
            if !version_entry.install_checks.trim().is_empty() {
                variant.install_checks = version_entry.install_checks.clone();
            }
            variant.versions = vec![version_entry];
            variant
        })
        .collect()
}

fn expand_plugin_list(plugins: Vec<PluginRaw>) -> Vec<PluginRaw> {
    plugins
        .into_iter()
        .flat_map(expand_plugin_variants)
        .collect()
}

fn plugin_download_urls(version: &PluginVersionRaw) -> Vec<String> {
    let mut urls = Vec::new();
    for candidate in std::iter::once(version.f_path.as_str())
        .chain(version.downloads.iter().map(String::as_str))
        .chain(version.mirrors.iter().map(String::as_str))
    {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !urls.iter().any(|existing| existing == trimmed) {
            urls.push(trimmed.to_string());
        }
    }
    urls
}

fn build_download_candidates(urls: &[String]) -> Vec<String> {
    let mut candidates = Vec::new();
    for url in urls {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !candidates.iter().any(|existing| existing == trimmed) {
            candidates.push(trimmed.to_string());
        }
        if let Some(fallback) = mysql_archive_cdn_fallback_url(trimmed) {
            if !candidates.iter().any(|existing| existing == &fallback) {
                candidates.push(fallback);
            }
        }
    }
    candidates
}

async fn load_plugin_store(force_refresh: bool) -> Result<PluginStoreFile, String> {
    let contents = sync_software_store(force_refresh).await?;
    serde_json::from_str::<PluginStoreFile>(&contents)
        .map_err(|error| format!("Invalid plugin.json payload: {error}"))
}

fn map_plugin_store(
    store: PluginStoreFile,
    base_dir: &Path,
    registry: &RuntimeRegistry,
) -> (Vec<SoftwareTypeEntry>, Vec<SoftwarePluginEntry>) {
    let mut type_map = HashMap::new();
    let mut types = store
        .r#type
        .into_iter()
        .filter(|entry| entry.id != 11)
        .collect::<Vec<_>>();
    types.sort_by(|left, right| {
        left.sort
            .cmp(&right.sort)
            .then_with(|| left.title.cmp(&right.title))
    });

    let software_types = types
        .iter()
        .map(|entry| {
            type_map.insert(entry.id, entry.title.clone());
            SoftwareTypeEntry {
                id: entry.id,
                title: entry.title.clone(),
            }
        })
        .collect::<Vec<_>>();

    let mut plugins = expand_plugin_list(store.list);
    plugins.sort_by(|left, right| left.sort.cmp(&right.sort));

    let runtime_entries = registry
        .entries
        .iter()
        .filter(|entry| is_runtime_entry_ready(entry))
        .collect::<Vec<_>>();

    let mut software_plugins = Vec::new();
    let mut consumed_runtime_ids = HashSet::new();
    for plugin in plugins {
        let runtime_kind = detect_runtime_kind(&plugin.name, &plugin.dependent);
        let plugin_version = select_plugin_version(&plugin);
        let mut matching_runtime_entries = runtime_entries
            .iter()
            .filter(|entry| entry.runtime_kind == runtime_kind)
            .collect::<Vec<_>>();
        matching_runtime_entries.sort_by(|left, right| right.version.cmp(&left.version));

        let has_same_version_installed = matching_runtime_entries
            .iter()
            .any(|entry| entry.version == plugin_version);

        if !has_same_version_installed {
            software_plugins.push(map_plugin_entry(
                plugin.clone(),
                &type_map,
                base_dir,
                registry,
            ));
        }

        for runtime_entry in matching_runtime_entries {
            if consumed_runtime_ids.insert(runtime_entry.id.clone()) {
                software_plugins.push(map_installed_runtime_entry(
                    runtime_entry,
                    Some(&plugin),
                    &type_map,
                ));
            }
        }
    }

    (software_types, software_plugins)
}

async fn sync_software_store(force_refresh: bool) -> Result<String, String> {
    let data_base_dir = resolve_data_base_dir()
        .ok_or_else(|| "Unable to resolve application directory".to_string())?;
    let resource_base_dir = resolve_resource_base_dir()
        .ok_or_else(|| "Unable to resolve application directory".to_string())?;
    let data_dir = data_base_dir.join("data");
    let plugin_path = data_dir.join("plugin.json");
    let bundled_plugin_path = resource_base_dir.join("data").join("plugin.json");

    if let Err(error) = fs::create_dir_all(&data_dir) {
        return Err(format!("Failed to create data directory: {error}"));
    }
    let legacy_sync_path = data_dir.join("plugin.sync");
    if legacy_sync_path.exists() {
        let _ = fs::remove_file(&legacy_sync_path);
    }

    if force_refresh {
        return refresh_software_store_now(&plugin_path, &bundled_plugin_path).await;
    }

    if let Some(cached_contents) = read_cached_software_store(&plugin_path, &bundled_plugin_path)? {
        if should_refresh_software_store(&plugin_path) {
            trigger_background_software_store_refresh(plugin_path.clone());
        }
        return Ok(cached_contents);
    }

    refresh_software_store_now(&plugin_path, &bundled_plugin_path).await
}

fn read_cached_software_store(
    plugin_path: &Path,
    bundled_plugin_path: &Path,
) -> Result<Option<String>, String> {
    if plugin_path.exists() {
        return fs::read_to_string(plugin_path)
            .map(Some)
            .map_err(|error| format!("Failed to read plugin cache: {error}"));
    }
    if bundled_plugin_path.exists() {
        return fs::read_to_string(bundled_plugin_path)
            .map(Some)
            .map_err(|error| format!("Failed to read bundled plugin list: {error}"));
    }
    Ok(None)
}

async fn refresh_software_store_now(
    plugin_path: &Path,
    bundled_plugin_path: &Path,
) -> Result<String, String> {
    match download_software_store().await {
        Ok(contents) => {
            validate_plugin_store(&contents)?;
            fs::write(plugin_path, &contents)
                .map_err(|error| format!("Failed to write plugin cache: {error}"))?;
            mark_software_refresh_checked_now();
            Ok(contents)
        }
        Err(error) if plugin_path.exists() => fs::read_to_string(plugin_path).map_err(|read_error| {
            format!("Failed to read cached plugin list after download error ({error}): {read_error}")
        }),
        Err(_) if bundled_plugin_path.exists() => fs::read_to_string(bundled_plugin_path)
            .map_err(|read_error| format!("Failed to read bundled plugin list: {read_error}")),
        Err(error) => Err(error),
    }
}

fn trigger_background_software_store_refresh(plugin_path: PathBuf) {
    let now = SystemTime::now();
    {
        let mut state = software_refresh_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.in_progress {
            return;
        }
        if let Some(last_check) = state.last_check {
            if now
                .duration_since(last_check)
                .map(|age| age < SOFTWARE_CACHE_TTL)
                .unwrap_or(false)
            {
                return;
            }
        }
        state.in_progress = true;
    }

    tokio::spawn(async move {
        let refresh_result = async {
            let contents = download_software_store().await?;
            validate_plugin_store(&contents)?;
            let current = fs::read_to_string(&plugin_path).unwrap_or_default();
            if current != contents {
                fs::write(&plugin_path, contents)
                    .map_err(|error| format!("Failed to write plugin cache: {error}"))?;
            }
            Ok::<(), String>(())
        }
        .await;

        if let Err(error) = refresh_result {
            eprintln!("software store background sync warning: {error}");
        }
        mark_software_refresh_checked_now();
    });
}

fn mark_software_refresh_checked_now() {
    let mut state = software_refresh_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.in_progress = false;
    state.last_check = Some(SystemTime::now());
}

fn default_software_store() -> (Vec<SoftwareTypeEntry>, Vec<SoftwarePluginEntry>) {
    (Vec::new(), Vec::new())
}

fn registry_php_options(registry: &RuntimeRegistry) -> Vec<PhpRuntimeOption> {
    let mut options = registry
        .entries
        .iter()
        .filter(|entry| entry.runtime_kind == "php" && is_runtime_entry_ready(entry))
        .map(|entry| PhpRuntimeOption {
            id: runtime_binding_id(entry),
            label: format!("PHP {}", entry.version),
            version: entry.version.clone(),
        })
        .collect::<Vec<_>>();
    options.sort_by(|left, right| right.version.cmp(&left.version));
    options
}

pub(crate) fn active_web_server_runtime(registry: &RuntimeRegistry) -> Option<WebServerRuntime> {
    registry
        .entries
        .iter()
        .filter(|entry| {
            matches!(entry.runtime_kind.as_str(), "apache" | "nginx")
                && is_runtime_entry_ready(entry)
        })
        .max_by_key(|entry| {
            (
                entry.state == "running",
                web_server_runtime_priority(&entry.runtime_kind),
                entry.version.clone(),
            )
        })
        .map(|entry| WebServerRuntime {
            kind: entry.runtime_kind.clone(),
            label: web_server_runtime_label(&entry.runtime_kind),
            version: entry.version.clone(),
            status: entry.state.clone(),
        })
}

fn web_server_runtime_priority(kind: &str) -> u8 {
    match kind {
        "apache" => 2,
        "nginx" => 1,
        _ => 0,
    }
}

fn web_server_runtime_label(kind: &str) -> String {
    match kind {
        "apache" => "Apache",
        "nginx" => "Nginx",
        _ => "Web Server",
    }
    .to_string()
}

fn build_runtime_id(name: &str, version: &str, runtime_kind: &str) -> String {
    format!(
        "{}-{}-{}",
        slugify(name, '-'),
        slugify(version, '-'),
        slugify(runtime_kind, '-')
    )
}

pub(crate) fn runtime_binding_id(entry: &InstalledRuntime) -> String {
    build_runtime_id(&entry.name, &entry.version, &entry.runtime_kind)
}

pub(crate) fn resolve_php_runtime_binding_id(
    binding_id: &str,
    php_registry: &HashMap<String, InstalledRuntime>,
) -> Option<String> {
    let binding_id = binding_id.trim();
    if binding_id.is_empty() {
        return None;
    }

    if php_registry.contains_key(binding_id) {
        return Some(binding_id.to_string());
    }

    let mut legacy_matches = php_registry
        .iter()
        .filter(|(_, entry)| {
            let legacy_version_binding = format!(
                "{}-{}",
                slugify(&entry.name, '-'),
                slugify(&entry.version, '-')
            );
            let version_only_binding = slugify(&entry.version, '-');

            entry.id == binding_id
                || slugify(&entry.name, '-') == binding_id
                || legacy_version_binding == binding_id
                || version_only_binding == binding_id
                || entry.version == binding_id
        })
        .map(|(id, entry)| (id.clone(), entry.version.clone()))
        .collect::<Vec<_>>();
    legacy_matches.sort_by(|left, right| right.1.cmp(&left.1));
    legacy_matches.into_iter().next().map(|(id, _)| id)
}

pub(crate) fn load_runtime_registry() -> Result<RuntimeRegistry, String> {
    let runtime_root = runtime_root_path()?;
    if !runtime_root.exists() {
        return Ok(RuntimeRegistry::default());
    }

    let inspection = RuntimeInspection::collect();
    let mut entries = Vec::new();
    for runtime_dir in fs::read_dir(&runtime_root)
        .map_err(|error| format!("Failed to read runtime root: {error}"))?
    {
        let runtime_dir =
            runtime_dir.map_err(|error| format!("Failed to access runtime root entry: {error}"))?;
        let runtime_path = runtime_dir.path();
        if !runtime_path.is_dir() {
            continue;
        }

        for version_dir in fs::read_dir(&runtime_path)
            .map_err(|error| format!("Failed to read runtime version directory: {error}"))?
        {
            let version_dir = version_dir
                .map_err(|error| format!("Failed to access runtime version entry: {error}"))?;
            let install_dir = version_dir.path();
            if !install_dir.is_dir() {
                continue;
            }
            let inferred = infer_runtime_entry(&install_dir, &inspection)?;
            entries.push(hydrate_runtime_entry(inferred, &install_dir, &inspection));
        }
    }

    Ok(RuntimeRegistry { entries })
}

pub(crate) fn save_runtime_registry(registry: &RuntimeRegistry) -> Result<(), String> {
    let path = runtime_registry_path()?;
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            return Err(format!("Failed to create registry directory: {error}"));
        }
    }
    let contents = serde_json::to_string_pretty(registry)
        .map_err(|error| format!("Failed to serialize registry: {error}"))?;
    fs::write(&path, contents)
        .map_err(|error| format!("Failed to write runtime registry: {error}"))?;
    Ok(())
}

fn runtime_root_path() -> Result<PathBuf, String> {
    if let Some(override_path) = resolve_env_path_override("MINPANEL_RUNTIME_ROOT") {
        return Ok(override_path);
    }
    let base_dir = resolve_data_base_dir()
        .ok_or_else(|| "Unable to resolve application directory".to_string())?;
    Ok(base_dir.join("data").join("runtime"))
}

fn runtime_registry_path() -> Result<PathBuf, String> {
    let base_dir = resolve_data_base_dir()
        .ok_or_else(|| "Unable to resolve application directory".to_string())?;
    Ok(base_dir.join("data").join("registry").join("software.json"))
}

fn database_credentials_path() -> Result<PathBuf, String> {
    let base_dir = resolve_data_base_dir()
        .ok_or_else(|| "Unable to resolve application directory".to_string())?;
    Ok(base_dir
        .join("data")
        .join("registry")
        .join("databases.json"))
}

fn hydrate_runtime_entry(
    mut entry: InstalledRuntime,
    install_dir: &Path,
    inspection: &RuntimeInspection,
) -> InstalledRuntime {
    entry.install_dir = install_dir.display().to_string();
    if entry.runtime_kind != "phpmyadmin" && entry.executable_path.is_none() {
        entry.executable_path = detect_runtime_executable(install_dir, &entry.runtime_kind, true)
            .map(|path| path.display().to_string());
    }
    entry.pid = detect_runtime_pid_with_inspection(&entry, inspection);
    if is_runtime_available_after_start_with_inspection(&entry, inspection) {
        entry.state = "running".to_string();
    } else if entry.state.is_empty() {
        entry.state = runtime_default_state(&entry.runtime_kind);
    } else {
        entry.state = runtime_default_state(&entry.runtime_kind);
    }

    entry
}

fn infer_runtime_entry(
    install_dir: &Path,
    inspection: &RuntimeInspection,
) -> Result<InstalledRuntime, String> {
    let version = install_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Runtime version directory is invalid".to_string())?
        .to_string();
    let name = install_dir
        .parent()
        .and_then(|value| value.file_name())
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Runtime name directory is invalid".to_string())?
        .to_string();
    let runtime_kind = detect_runtime_kind(&name, "");
    let executable_path = if runtime_kind == "phpmyadmin" {
        None
    } else {
        detect_runtime_executable(install_dir, &runtime_kind, true)
            .map(|path| path.display().to_string())
    };
    let php_port = (runtime_kind == "php")
        .then(|| resolve_php_runtime_port(install_dir, &version, inspection));
    cleanup_legacy_runtime_metadata(install_dir);

    Ok(InstalledRuntime {
        id: build_runtime_id(&name, &version, &runtime_kind),
        name: name.clone(),
        title: name.clone(),
        version,
        runtime_kind: runtime_kind.clone(),
        install_dir: install_dir.display().to_string(),
        package_file: String::new(),
        executable_path,
        state: runtime_default_state(&runtime_kind),
        pid: None,
        php_port,
    })
}

#[cfg(windows)]
fn uninstall_native_windows_runtime(entry: &InstalledRuntime) -> Result<String, String> {
    run_native_windows_runtime_action(
        "uninstall",
        Path::new(&entry.install_dir),
        &entry.runtime_kind,
        entry.php_port,
    )
    .unwrap_or_else(|| Ok(format!("No runtime uninstaller for {}", entry.runtime_kind)))
}

fn extract_plugin_package_archive(
    package_path: &Path,
    install_dir: &Path,
    runtime_kind: &str,
) -> Result<(), String> {
    if let Err(native_error) = extract_plugin_package_archive_with_tar(package_path, install_dir) {
        return Err(native_error);
    }

    flatten_extracted_runtime_root(install_dir, runtime_kind)?;

    Ok(())
}

fn extract_plugin_package_archive_with_tar(
    package_path: &Path,
    install_dir: &Path,
) -> Result<(), String> {
    let mut command = Command::new("tar");
    let output = hide_windows_console_window(
        command
            .arg("-xf")
            .arg(package_path)
            .arg("-C")
            .arg(install_dir),
    )
    .output()
    .map_err(|error| format!("tar extract failed to start: {error}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("tar exited with status {}", output.status)
    };
    Err(detail)
}

fn flatten_extracted_runtime_root(install_dir: &Path, runtime_kind: &str) -> Result<(), String> {
    if is_runtime_root_layout(install_dir, runtime_kind) {
        return Ok(());
    }

    let Some(nested_root) = find_nested_runtime_root(install_dir, runtime_kind) else {
        return Ok(());
    };

    move_directory_contents(&nested_root, install_dir)?;
    remove_empty_directory_chain(Some(nested_root.as_path()), install_dir)?;

    Ok(())
}

fn find_nested_runtime_root(search_root: &Path, runtime_kind: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(search_root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if is_runtime_root_layout(&path, runtime_kind) {
            return Some(path);
        }
        if let Some(found) = find_nested_runtime_root(&path, runtime_kind) {
            return Some(found);
        }
    }
    None
}

fn move_directory_contents(source_dir: &Path, target_dir: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source_dir)
        .map_err(|error| format!("Failed to inspect nested runtime directory: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("Failed to read nested runtime entry: {error}"))?;
        let source_path = entry.path();
        let target_path = target_dir.join(entry.file_name());
        if target_path.exists() {
            continue;
        }
        fs::rename(&source_path, &target_path)
            .map_err(|error| format!("Failed to normalize extracted runtime layout: {error}"))?;
    }

    Ok(())
}

fn remove_empty_directory_chain(start_dir: Option<&Path>, stop_dir: &Path) -> Result<(), String> {
    let mut current = start_dir.map(Path::to_path_buf);
    while let Some(path) = current {
        if path == stop_dir {
            break;
        }
        let is_empty = fs::read_dir(&path)
            .map_err(|error| format!("Failed to verify nested runtime directory: {error}"))?
            .next()
            .is_none();
        if !is_empty {
            break;
        }
        current = path.parent().map(Path::to_path_buf);
        fs::remove_dir(&path)
            .map_err(|error| format!("Failed to remove nested runtime directory: {error}"))?;
    }

    Ok(())
}

fn is_runtime_root_layout(base_dir: &Path, runtime_kind: &str) -> bool {
    match runtime_kind {
        "apache" => base_dir.join("conf").join("httpd.conf").exists(),
        "phpmyadmin" => base_dir.join("index.php").exists(),
        _ => detect_runtime_executable(base_dir, runtime_kind, false).is_some(),
    }
}

fn runtime_default_state(runtime_kind: &str) -> String {
    if runtime_kind == "phpmyadmin" {
        "ready".to_string()
    } else {
        "stopped".to_string()
    }
}

fn runtime_uses_lua_plugin(runtime_kind: &str) -> bool {
    matches!(runtime_kind, "apache" | "php" | "mysql" | "phpmyadmin")
}

fn should_refresh_software_store(plugin_path: &Path) -> bool {
    if !plugin_path.exists() {
        return true;
    }

    let modified = plugin_path
        .metadata()
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    match SystemTime::now().duration_since(modified) {
        Ok(age) => age >= SOFTWARE_CACHE_TTL,
        Err(_) => true,
    }
}

async fn download_plugin_package(plugin_id: &str) -> Result<PathBuf, String> {
    let bundle = download_plugin_bundle_with_task(plugin_id, "").await?;
    Ok(bundle.package_file)
}

async fn install_plugin_package(plugin_id: &str, task_id: &str) -> Result<String, String> {
    let data_base_dir = resolve_data_base_dir()
        .ok_or_else(|| "Unable to resolve application directory".to_string())?;

    let (_plugin, _version_entry, version, runtime_kind) =
        resolve_plugin_definition(plugin_id).await?;

    let php_port = if runtime_kind == "php" {
        Some(php_fastcgi_port(&version))
    } else {
        None
    };

    let bundle = download_plugin_bundle_with_task(plugin_id, task_id).await?;
    let mut registry = load_runtime_registry().unwrap_or_default();
    let install_root = data_base_dir
        .join("data")
        .join("runtime")
        .join(sanitize_path_segment(&bundle.plugin_name))
        .join(sanitize_path_segment(&bundle.version));

    let runtime_id = build_runtime_id(&bundle.plugin_name, &bundle.version, &bundle.runtime_kind);
    let _ = stop_installed_runtime(&runtime_id);

    if install_root.exists() {
        update_task_log(task_id, "Removing previous installation...");
        // Robust deletion with retry for Windows
        let mut deleted = false;
        for i in 0..5 {
            match fs::remove_dir_all(&install_root) {
                Ok(_) => {
                    deleted = true;
                    break;
                }
                Err(error) => {
                    if i < 4 {
                        update_task_log(
                            task_id,
                            &format!("Retrying directory removal (attempt {}/5)...", i + 2),
                        );
                        thread::sleep(Duration::from_millis(500));
                    } else {
                        // Final fallback: attempt to rename it out of the way
                        let trash_dir = data_base_dir
                            .join("data")
                            .join("trash")
                            .join(uuid::Uuid::new_v4().to_string());
                        let _ = fs::create_dir_all(trash_dir.parent().unwrap());
                        if fs::rename(&install_root, &trash_dir).is_ok() {
                            let _ = fs::remove_dir_all(&trash_dir);
                            deleted = true;
                        } else {
                            return Err(format!(
                                "Failed to replace previous install directory: {error}"
                            ));
                        }
                    }
                }
            }
        }
        if !deleted {
            return Err(
                "Failed to clear installation directory after multiple attempts.".to_string(),
            );
        }
    }
    fs::create_dir_all(&install_root)
        .map_err(|error| format!("Failed to create install directory: {error}"))?;

    update_task_log(task_id, "0% giải nén tệp .zip");
    if let Err(error) =
        extract_plugin_package_archive(&bundle.package_file, &install_root, &bundle.runtime_kind)
    {
        update_task_log(task_id, &format!("Extraction failed: {error}"));
        let _ = fs::remove_dir_all(&install_root);
        return Err(error);
    }
    update_task_log(task_id, "100% giải nén tệp .zip");

    cleanup_legacy_runtime_metadata(&install_root);

    if runtime_uses_lua_plugin(&bundle.runtime_kind) {
        update_task_log(
            task_id,
            &format!(
                "running {} setup scripts...",
                bundle.runtime_kind.to_uppercase()
            ),
        );
        if let Err(error) = run_native_windows_runtime_action(
            "install",
            &install_root,
            &bundle.runtime_kind,
            php_port,
        )
        .unwrap_or(Ok(String::new()))
        {
            update_task_log(task_id, &format!("Setup failed: {error}"));
            let _ = fs::remove_dir_all(&install_root);
            return Err(format!(
                "{} installed files but setup failed: {error}",
                bundle.runtime_kind.to_uppercase()
            ));
        }
        update_task_log(task_id, "setup complete.");
    }

    let executable_path = detect_runtime_executable(&install_root, &bundle.runtime_kind, true)
        .map(|path| path.display().to_string());
    if bundle.runtime_kind != "phpmyadmin" && executable_path.is_none() {
        let mut contents = Vec::new();
        if let Ok(entries) = fs::read_dir(&install_root) {
            for entry in entries.flatten() {
                contents.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        let dir_info = if contents.is_empty() {
            " (directory is empty)".to_string()
        } else {
            format!(": [{}]", contents.join(", "))
        };
        let _ = fs::remove_dir_all(&install_root);
        return Err(format!(
            "Install failed: runtime executable was not found after extraction{dir_info}"
        ));
    }

    let mut installed_entry = InstalledRuntime {
        id: build_runtime_id(&bundle.plugin_name, &bundle.version, &bundle.runtime_kind),
        name: bundle.plugin_name.clone(),
        title: bundle.plugin_title,
        version: bundle.version.clone(),
        runtime_kind: bundle.runtime_kind.clone(),
        install_dir: install_root.display().to_string(),
        package_file: bundle.package_file.display().to_string(),
        executable_path,
        state: runtime_default_state(&bundle.runtime_kind),
        pid: None,
        php_port,
    };

    upsert_runtime_entry(&mut registry, installed_entry.clone());
    let _ = save_runtime_registry(&registry);

    if matches!(bundle.runtime_kind.as_str(), "apache" | "php" | "mysql") {
        if let Err(error) = sync_apache_site_bindings(&mut registry) {
            if bundle.runtime_kind == "apache" {
                return Err(format!(
                    "Apache installed but could not finish startup: {error}"
                ));
            }
        }
        if let Err(error) = ensure_runtime_start_preconditions(&installed_entry) {
            return Err(format!(
                "{} installed but could not start: {error}",
                bundle.runtime_kind.to_uppercase()
            ));
        }
        if let Err(error) = run_native_windows_runtime_action(
            "start",
            &install_root,
            &bundle.runtime_kind,
            php_port,
        )
        .unwrap_or(Ok(String::new()))
        {
            return Err(format!(
                "{} installed but could not start: {error}",
                bundle.runtime_kind.to_uppercase()
            ));
        }
        if !wait_for_runtime_start(&mut installed_entry, Duration::from_secs(10)) {
            let detail = runtime_start_failure_detail(&installed_entry)
                .map(|detail| format!(": {detail}"))
                .unwrap_or_default();
            return Err(format!(
                "{} installed but start failed{}",
                bundle.runtime_kind.to_uppercase(),
                detail
            ));
        }
        upsert_runtime_entry(&mut registry, installed_entry.clone());
        let _ = save_runtime_registry(&registry);
    } else {
        // For tools like PHPMyAdmin that don't "start"
        let _ = save_runtime_registry(&registry);
    }

    let action_summary = if matches!(bundle.runtime_kind.as_str(), "apache" | "php" | "mysql") {
        "Installed and started"
    } else {
        "Installed"
    };
    Ok(format!(
        "{action_summary} {} {} natively",
        bundle.runtime_kind.to_uppercase(),
        bundle.version
    ))
}

async fn resolve_plugin_definition(
    plugin_id: &str,
) -> Result<(PluginRaw, Option<PluginVersionRaw>, String, String), String> {
    let store = load_plugin_store(false).await?;
    let plugin = expand_plugin_list(store.list)
        .into_iter()
        .find(|entry| {
            let version = select_plugin_version(entry);
            let runtime_kind = detect_runtime_kind(&entry.name, &entry.dependent);
            let id = build_runtime_id(&entry.name, &version, &runtime_kind);
            id == plugin_id
        })
        .ok_or_else(|| "Plugin not found in software store".to_string())?;
    let version_entry = plugin
        .versions
        .iter()
        .find(|entry| !plugin_download_urls(entry).is_empty())
        .cloned();
    let version = select_plugin_version(&plugin);
    let runtime_kind = detect_runtime_kind(&plugin.name, &plugin.dependent);
    Ok((plugin, version_entry, version, runtime_kind))
}

async fn download_plugin_bundle_with_task(
    plugin_id: &str,
    task_id: &str,
) -> Result<DownloadedPluginBundle, String> {
    let (plugin, version_entry, version, runtime_kind) =
        resolve_plugin_definition(plugin_id).await?;
    let data_base_dir = resolve_data_base_dir()
        .ok_or_else(|| "Unable to resolve application directory".to_string())?;
    let downloads_dir = data_base_dir.join("data").join("downloads");
    if !downloads_dir.exists() {
        let _ = fs::create_dir_all(&downloads_dir);
    }
    let package_urls = version_entry
        .as_ref()
        .map(plugin_download_urls)
        .unwrap_or_default();
    if package_urls.is_empty() {
        return Err("No package found for this version".to_string());
    }
    let file_name = format!(
        "{}-{}.zip",
        sanitize_path_segment(&plugin.name),
        sanitize_path_segment(&version)
    );
    let target_path = downloads_dir.join(file_name);
    download_urls_to_path_with_task(&package_urls, &target_path, "plugin package", task_id).await?;
    Ok(DownloadedPluginBundle {
        plugin_name: plugin.name,
        plugin_title: plugin.title,
        version,
        runtime_kind,
        package_file: target_path,
    })
}

async fn download_urls_to_path_with_task(
    urls: &[String],
    target_path: &Path,
    asset_label: &str,
    task_id: &str,
) -> Result<(), String> {
    let candidates = build_download_candidates(urls);
    if candidates.is_empty() {
        return Err(format!("No download URL configured for {asset_label}"));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) MinPanel/0.1")
        .build()
        .map_err(|error| format!("Failed to create HTTP client: {error}"))?;

    let mut last_error = None;
    for (index, url) in candidates.iter().enumerate() {
        if index > 0 {
            update_task_log(
                task_id,
                &format!("Retrying download from mirror {index}..."),
            );
        }
        match download_url_to_path_with_client(&client, url, target_path, asset_label, task_id)
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                let _ = std::fs::remove_file(target_path);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| format!("Failed to download {asset_label}")))
}

async fn download_url_to_path_with_client(
    client: &reqwest::Client,
    url: &str,
    target_path: &Path,
    asset_label: &str,
    task_id: &str,
) -> Result<(), String> {
    let mut response = send_download_request(client, url)
        .await
        .map_err(|error| format!("Failed to download {asset_label}: {error} ({url})"))?;
    let effective_url = url.to_string();

    if !response.status().is_success() {
        return Err(format!(
            "Download failed with status: {} ({effective_url})",
            response.status()
        ));
    }

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded = 0;
    let mut last_percent = 0;

    let mut file = File::create(target_path)
        .await
        .map_err(|e| format!("Failed to create file: {e}"))?;

    update_task_log(task_id, "0% tải xuống tệp .zip");

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Error downloading: {e}"))?
    {
        downloaded += chunk.len() as u64;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Error writing: {e}"))?;

        if total_size > 0 {
            let percent = (downloaded * 100 / total_size) as u32;
            if percent >= last_percent + 5 || percent == 100 {
                update_task_log(task_id, &format!("{}% tải xuống tệp .zip", percent));
                last_percent = percent;
            }
        }
    }

    Ok(())
}

async fn send_download_request(
    client: &reqwest::Client,
    url: &str,
) -> Result<reqwest::Response, reqwest::Error> {
    let mut request = client
        .get(url)
        .header(
            reqwest::header::ACCEPT,
            "application/zip, application/octet-stream, */*",
        )
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9");
    if url.contains("downloads.mysql.com") || url.contains("cdn.mysql.com") {
        request = request.header(reqwest::header::REFERER, "https://downloads.mysql.com/");
    }
    request.send().await
}

fn mysql_archive_cdn_fallback_url(url: &str) -> Option<String> {
    if !url.contains("downloads.mysql.com/archives/get/") {
        return None;
    }
    let file_name = url.split("/file/").nth(1)?.split(['?', '#']).next()?.trim();
    let version = file_name.strip_prefix("mysql-")?.split('-').next()?;
    let mut parts = version.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    if major.is_empty() || minor.is_empty() {
        return None;
    }
    Some(format!(
        "https://cdn.mysql.com/archives/mysql-{major}.{minor}/{file_name}"
    ))
}

fn run_native_windows_runtime_action(
    action: &str,
    install_dir: &Path,
    runtime_kind: &str,
    php_port: Option<u16>,
) -> Option<Result<String, String>> {
    #[cfg(windows)]
    {
        let engine = LuaPluginEngine::new();
        let hook_name = match action {
            "install" => "on_install",
            "start" => "on_start",
            "stop" => "on_stop",
            "uninstall" => "on_uninstall",
            _ => return None,
        };

        let mut ctx = HashMap::new();
        ctx.insert(
            "install_dir".to_string(),
            install_dir.to_string_lossy().to_string(),
        );
        ctx.insert("runtime_kind".to_string(), runtime_kind.to_string());
        if let Some(base_dir) = resolve_data_base_dir() {
            ctx.insert(
                "data_root".to_string(),
                base_dir.join("data").display().to_string(),
            );
        }
        ctx.insert(
            "website_root".to_string(),
            website::resolve_website_root().display().to_string(),
        );
        if let Some(port) = php_port {
            ctx.insert("port".to_string(), port.to_string());
        }
        if runtime_kind == "apache" {
            if let Ok(registry) = load_runtime_registry() {
                if let Some(phpmyadmin) = select_primary_runtime(&registry, "phpmyadmin") {
                    ctx.insert("phpmyadmin_dir".to_string(), phpmyadmin.install_dir.clone());
                }
                if let Some(port) =
                    select_primary_runtime(&registry, "php").and_then(|entry| entry.php_port)
                {
                    ctx.insert("phpmyadmin_php_port".to_string(), port.to_string());
                }
            }
        }

        Some(engine.call_hook(runtime_kind, hook_name, ctx))
    }
    #[cfg(not(windows))]
    {
        let _ = (action, install_dir, runtime_kind, php_port);
        None
    }
}

#[cfg(not(windows))]
fn supports_native_windows_runtime_action(_runtime_kind: &str, _action: &str) -> bool {
    false
}

fn hide_windows_console_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(windows)]
fn force_kill_windows_pid(pid: u32) -> Result<(), String> {
    let mut command = Command::new("taskkill");
    let output = hide_windows_console_window(
        command
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped()),
    )
    .output()
    .map_err(|error| format!("Failed to terminate process {pid}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.contains("not found") || stderr.contains("There is no running instance") {
        return Ok(());
    }
    Err(if stderr.is_empty() {
        format!("Failed to terminate process {pid}")
    } else {
        format!("Failed to terminate process {pid}: {stderr}")
    })
}

fn wait_for_condition(
    timeout: Duration,
    interval: Duration,
    mut predicate: impl FnMut() -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if predicate() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(interval);
    }
}

fn refresh_runtime_started_state(entry: &mut InstalledRuntime) -> bool {
    let inspection = RuntimeInspection::collect();
    entry.pid = detect_runtime_pid_with_inspection(entry, &inspection);
    let available = entry.runtime_kind == "phpmyadmin"
        || is_runtime_available_after_start_with_inspection(entry, &inspection);
    entry.state = if entry.runtime_kind == "phpmyadmin" {
        "ready".to_string()
    } else if available {
        "running".to_string()
    } else {
        runtime_default_state(&entry.runtime_kind)
    };
    available
}

fn wait_for_runtime_start(entry: &mut InstalledRuntime, timeout: Duration) -> bool {
    let _ = wait_for_condition(timeout, Duration::from_millis(250), || {
        let inspection = RuntimeInspection::collect();
        entry.pid = detect_runtime_pid_with_inspection(entry, &inspection);
        is_runtime_available_after_start_with_inspection(entry, &inspection)
    });
    refresh_runtime_started_state(entry)
}

fn runtime_start_failure_detail(entry: &InstalledRuntime) -> Option<String> {
    match entry.runtime_kind.as_str() {
        "apache" => apache_start_failure_detail(entry),
        _ => None,
    }
}

fn ensure_runtime_start_preconditions(entry: &InstalledRuntime) -> Result<(), String> {
    match entry.runtime_kind.as_str() {
        "apache" => {
            if let Some(detail) = apache_port_conflict_detail(entry) {
                return Err(detail);
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn apache_start_failure_detail(entry: &InstalledRuntime) -> Option<String> {
    join_unique_details(vec![
        apache_port_conflict_detail(entry),
        apache_error_log_detail(Path::new(&entry.install_dir)),
    ])
}

fn apache_error_log_detail(install_dir: &Path) -> Option<String> {
    let mut candidates = vec![install_dir.join("logs").join("error.log")];
    if let Some(base_dir) = resolve_data_base_dir() {
        candidates.push(
            base_dir
                .join("data")
                .join("logs")
                .join("apache")
                .join("error.log"),
        );
    }

    candidates
        .into_iter()
        .find_map(|path| read_last_actionable_line(&path))
}

fn join_unique_details(details: Vec<Option<String>>) -> Option<String> {
    let mut unique = Vec::new();
    let mut seen = HashSet::new();
    for detail in details.into_iter().flatten() {
        let trimmed = detail.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = trimmed.to_ascii_lowercase();
        if seen.insert(normalized) {
            unique.push(trimmed.to_string());
        }
    }

    if unique.is_empty() {
        None
    } else {
        Some(unique.join(" | "))
    }
}

fn read_last_actionable_line(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    for line in contents
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if apache_log_line_is_actionable(line) {
            return Some(line.to_string());
        }
    }
    None
}

#[cfg(not(windows))]
fn is_tcp_port_open(target: (&str, u16)) -> bool {
    TcpStream::connect_timeout(
        &format!("{}:{}", target.0, target.1)
            .parse()
            .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], target.1))),
        Duration::from_millis(250),
    )
    .is_ok()
}

fn apache_log_line_is_actionable(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "address already in use",
        "bind to address",
        "could not",
        "denied",
        "error",
        "failed",
        "make_sock",
        "no listening sockets available",
        "unable",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}
fn find_running_pids_by_executable_in_system(executable_path: &Path, system: &System) -> Vec<u32> {
    system
        .processes()
        .values()
        .filter_map(|process| {
            process
                .exe()
                .filter(|path| paths_match_for_process(path, executable_path))
                .map(|_| process.pid().as_u32())
        })
        .collect()
}

fn find_running_pid_by_executable_in_system(
    executable_path: &Path,
    system: &System,
) -> Option<u32> {
    find_running_pids_by_executable_in_system(executable_path, system)
        .into_iter()
        .min()
}

fn paths_match_for_process(candidate: &Path, expected: &Path) -> bool {
    let candidate_normalized = normalize_process_path(candidate);
    let expected_normalized = normalize_process_path(expected);
    candidate_normalized == expected_normalized
}

fn normalize_process_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn process_path_matches_install_dir(path: &Path, install_dir: &Path) -> bool {
    let path_normalized = normalize_process_path(path);
    let install_normalized = normalize_process_path(install_dir)
        .trim_end_matches('/')
        .to_string();
    path_normalized == install_normalized
        || path_normalized.starts_with(&(install_normalized.clone() + "/"))
}

fn detect_runtime_pid(entry: &InstalledRuntime) -> Option<u32> {
    let inspection = RuntimeInspection::collect();
    detect_runtime_pid_with_inspection(entry, &inspection)
}

fn detect_runtime_pid_with_inspection(
    entry: &InstalledRuntime,
    inspection: &RuntimeInspection,
) -> Option<u32> {
    let install_dir = Path::new(&entry.install_dir);
    read_runtime_pid_value(install_dir, &entry.runtime_kind)
        .filter(|pid| runtime_pid_is_active_in_system(*pid, entry, &inspection.system))
        .or_else(|| {
            entry
                .executable_path
                .as_deref()
                .map(Path::new)
                .and_then(|path| find_running_pid_by_executable_in_system(path, &inspection.system))
        })
        .or(entry
            .pid
            .filter(|pid| runtime_pid_is_active_in_system(*pid, entry, &inspection.system)))
}

fn is_runtime_available_after_start_with_inspection(
    entry: &InstalledRuntime,
    inspection: &RuntimeInspection,
) -> bool {
    match entry.runtime_kind.as_str() {
        "phpmyadmin" => true,
        "apache" | "php" => runtime_listener_is_active_with_inspection(entry, inspection),
        _ => detect_runtime_pid_with_inspection(entry, inspection).is_some(),
    }
}

#[cfg(windows)]
#[allow(dead_code)]
fn runtime_listener_is_active(entry: &InstalledRuntime) -> bool {
    let inspection = RuntimeInspection::collect();
    runtime_listener_is_active_with_inspection(entry, &inspection)
}

#[cfg(windows)]
fn runtime_listener_is_active_with_inspection(
    entry: &InstalledRuntime,
    inspection: &RuntimeInspection,
) -> bool {
    let install_dir = Path::new(&entry.install_dir);
    let process_names = runtime_process_names(&entry.runtime_kind);
    if process_names.is_empty() {
        return false;
    }

    let port = match entry.runtime_kind.as_str() {
        "apache" => 80,
        "php" => match entry.php_port {
            Some(port) => port,
            None => return false,
        },
        _ => return false,
    };

    inspection
        .listening_tcp_pids
        .get(&port)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .any(|pid| {
            pid_matches_runtime_in_system(pid, process_names, install_dir, &inspection.system)
        })
}

#[cfg(not(windows))]
#[allow(dead_code)]
fn runtime_listener_is_active(entry: &InstalledRuntime) -> bool {
    let inspection = RuntimeInspection::collect();
    runtime_listener_is_active_with_inspection(entry, &inspection)
}

#[cfg(not(windows))]
fn runtime_listener_is_active_with_inspection(
    entry: &InstalledRuntime,
    _inspection: &RuntimeInspection,
) -> bool {
    match entry.runtime_kind.as_str() {
        "apache" => is_tcp_port_open(("127.0.0.1", 80)),
        "php" => entry
            .php_port
            .map(|port| is_tcp_port_open(("127.0.0.1", port)))
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(windows)]
#[allow(dead_code)]
fn runtime_pid_is_active(pid: u32, entry: &InstalledRuntime) -> bool {
    let system = collect_process_system();
    runtime_pid_is_active_in_system(pid, entry, &system)
}

#[cfg(not(windows))]
#[allow(dead_code)]
fn runtime_pid_is_active(pid: u32, _entry: &InstalledRuntime) -> bool {
    let system = collect_process_system();
    is_process_running_in_system(pid, &system)
}

#[cfg(windows)]
fn runtime_pid_is_active_in_system(pid: u32, entry: &InstalledRuntime, system: &System) -> bool {
    is_process_running_in_system(pid, system)
        && pid_matches_runtime_in_system(
            pid,
            runtime_process_names(&entry.runtime_kind),
            Path::new(&entry.install_dir),
            system,
        )
}

#[cfg(not(windows))]
fn runtime_pid_is_active_in_system(pid: u32, _entry: &InstalledRuntime, system: &System) -> bool {
    is_process_running_in_system(pid, system)
}

fn start_installed_runtime(runtime_id: &str) -> Result<String, String> {
    let mut registry = load_runtime_registry()?;
    let index = registry
        .entries
        .iter()
        .position(|entry| entry.id == runtime_id)
        .ok_or_else(|| "Runtime is not installed".to_string())?;
    let existing_pid = registry.entries[index].pid;
    if let Some(pid) = existing_pid {
        let inspection = RuntimeInspection::collect();
        if runtime_pid_is_active_in_system(pid, &registry.entries[index], &inspection.system)
            && is_runtime_available_after_start_with_inspection(
                &registry.entries[index],
                &inspection,
            )
        {
            registry.entries[index].state = "running".to_string();
            save_runtime_registry(&registry)?;
            return Ok("Runtime is already running".to_string());
        }
        registry.entries[index].pid = None;
    }

    if registry.entries[index].runtime_kind == "apache" {
        sync_apache_site_bindings(&mut registry)?;
    }

    let entry = &registry.entries[index];
    ensure_runtime_start_preconditions(entry)?;
    let install_dir = Path::new(&entry.install_dir);
    let message = if let Some(result) =
        run_native_windows_runtime_action("start", install_dir, &entry.runtime_kind, entry.php_port)
    {
        result?
    } else {
        format!(
            "Native start action for {} is not supported",
            entry.runtime_kind
        )
    };
    let (runtime_kind, missing_after_start) = {
        let entry = &mut registry.entries[index];
        let available = wait_for_runtime_start(entry, Duration::from_secs(10));
        (
            entry.runtime_kind.clone(),
            entry.runtime_kind != "phpmyadmin" && !available,
        )
    };
    save_runtime_registry(&registry)?;
    if runtime_kind != "phpmyadmin" && missing_after_start {
        let detail = runtime_start_failure_detail(&registry.entries[index])
            .map(|detail| format!(": {detail}"))
            .unwrap_or_default();
        return Err(format!(
            "Runtime start script completed but the runtime was not detected{detail}"
        ));
    }
    Ok(message)
}

fn stop_installed_runtime(runtime_id: &str) -> Result<String, String> {
    let mut registry = load_runtime_registry()?;
    let index = registry
        .entries
        .iter()
        .position(|entry| entry.id == runtime_id)
        .ok_or_else(|| "Runtime is not installed".to_string())?;
    {
        let entry = &mut registry.entries[index];
        entry.pid = detect_runtime_pid(entry);
        if entry.pid.is_none() && entry.state != "running" {
            entry.state = runtime_default_state(&entry.runtime_kind);
            if matches!(entry.runtime_kind.as_str(), "apache" | "php") {
                cleanup_runtime_pid_file(Path::new(&entry.install_dir), &entry.runtime_kind);
            }
            save_runtime_registry(&registry)?;
            return Ok("Runtime is already stopped".to_string());
        }
    }
    let entry = &registry.entries[index];
    let install_dir = Path::new(&entry.install_dir);
    let message = if let Some(result) =
        run_native_windows_runtime_action("stop", install_dir, &entry.runtime_kind, entry.php_port)
    {
        result?
    } else {
        format!(
            "Native stop action for {} is not supported",
            entry.runtime_kind
        )
    };
    thread::sleep(Duration::from_millis(500));
    let entry = &mut registry.entries[index];
    entry.pid = detect_runtime_pid(entry);
    if entry.runtime_kind != "phpmyadmin" && entry.pid.is_some() {
        return Err(
            "Runtime stop script completed but the runtime process is still running".to_string(),
        );
    }
    entry.pid = None;
    entry.state = runtime_default_state(&entry.runtime_kind);
    if matches!(entry.runtime_kind.as_str(), "apache" | "php") {
        cleanup_runtime_pid_file(Path::new(&entry.install_dir), &entry.runtime_kind);
    }
    save_runtime_registry(&registry)?;
    Ok(message)
}

#[allow(dead_code)]
pub(crate) fn stop_all_runtimes() -> Result<(), String> {
    let mut entries = load_runtime_registry().unwrap_or_default().entries;
    entries.sort_by(|left, right| {
        runtime_stop_priority(&left.runtime_kind)
            .cmp(&runtime_stop_priority(&right.runtime_kind))
            .then_with(|| left.version.cmp(&right.version))
    });

    let mut errors = Vec::new();
    for entry in entries {
        if entry.runtime_kind == "phpmyadmin" {
            continue;
        }
        if let Err(error) = stop_installed_runtime(&entry.id) {
            errors.push(format!("{} {}: {error}", entry.runtime_kind, entry.version));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub(crate) fn stop_all_runtimes_fast() -> Result<(), String> {
    #[cfg(windows)]
    {
        return stop_all_runtimes_fast_windows();
    }

    #[cfg(not(windows))]
    {
        stop_all_runtimes()
    }
}

#[cfg(windows)]
fn stop_all_runtimes_fast_windows() -> Result<(), String> {
    let mut registry = load_runtime_registry().unwrap_or_default();
    registry.entries.sort_by(|left, right| {
        runtime_stop_priority(&left.runtime_kind)
            .cmp(&runtime_stop_priority(&right.runtime_kind))
            .then_with(|| left.version.cmp(&right.version))
    });

    let mut errors = Vec::new();
    for entry in &registry.entries {
        if entry.runtime_kind == "phpmyadmin" {
            continue;
        }
        if let Err(error) = force_stop_runtime_quick(entry) {
            errors.push(format!("{} {}: {error}", entry.runtime_kind, entry.version));
        }
    }

    for entry in &mut registry.entries {
        if entry.runtime_kind == "phpmyadmin" {
            continue;
        }
        entry.pid = None;
        entry.state = runtime_default_state(&entry.runtime_kind);
        if matches!(entry.runtime_kind.as_str(), "apache" | "php") {
            let _ = fs::remove_file(runtime_pid_file(
                Path::new(&entry.install_dir),
                &entry.runtime_kind,
            ));
        }
    }
    let _ = save_runtime_registry(&registry);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(windows)]
fn force_stop_runtime_quick(entry: &InstalledRuntime) -> Result<(), String> {
    let install_dir = Path::new(&entry.install_dir);
    let process_names = runtime_process_names(&entry.runtime_kind);
    if process_names.is_empty() {
        return Ok(());
    }

    let _ = force_stop_processes_in_install_dir(install_dir, process_names);

    let pids = find_running_pids_for_runtime(entry);
    for pid in pids {
        force_kill_windows_pid(pid)?;
    }

    let stopped = wait_for_condition(
        Duration::from_millis(900),
        Duration::from_millis(100),
        || find_running_pids_for_runtime(entry).is_empty(),
    );
    if stopped {
        Ok(())
    } else {
        Err("runtime process is still running after fast shutdown".to_string())
    }
}

#[cfg(windows)]
fn runtime_process_names(runtime_kind: &str) -> &'static [&'static str] {
    match runtime_kind {
        "apache" => &["httpd.exe"],
        "php" => &["php-cgi.exe", "php.exe"],
        "mysql" => &["mysqld.exe"],
        _ => &[],
    }
}

#[cfg(windows)]
fn force_stop_processes_in_install_dir(
    install_dir: &Path,
    process_names: &[&str],
) -> Result<(), String> {
    if process_names.is_empty() {
        return Ok(());
    }

    let mut system = System::new_all();
    system.refresh_all();

    let mut errors = Vec::new();
    for process in system.processes().values() {
        let exe = process.exe();
        let Some(exe) = exe else {
            continue;
        };
        if !exe.starts_with(install_dir) {
            continue;
        }

        let file_name = exe
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let name_matches = process_names
            .iter()
            .any(|expected| process.name().eq_ignore_ascii_case(expected));
        let file_name_matches = process_names
            .iter()
            .any(|expected| file_name.eq_ignore_ascii_case(expected));
        if !(name_matches || file_name_matches) {
            continue;
        }

        if let Err(error) = force_kill_windows_pid(process.pid().as_u32()) {
            errors.push(error);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(windows)]
fn find_running_pids_for_runtime(entry: &InstalledRuntime) -> Vec<u32> {
    let install_dir = Path::new(&entry.install_dir);
    let system = collect_process_system();
    let mut pids = Vec::new();

    if let Some(pid) = read_runtime_pid_value(install_dir, &entry.runtime_kind)
        .filter(|pid| runtime_pid_is_active_in_system(*pid, entry, &system))
    {
        pids.push(pid);
    }
    if let Some(executable) = detect_runtime_executable(install_dir, &entry.runtime_kind, true) {
        pids.extend(find_running_pids_by_executable_in_system(
            &executable,
            &system,
        ));
    }

    let process_names = runtime_process_names(&entry.runtime_kind);
    if !process_names.is_empty() {
        for process in system.processes().values() {
            let pid = process.pid().as_u32();
            let Some(path) = process.exe() else {
                continue;
            };
            if !process_path_matches_install_dir(path, install_dir) {
                continue;
            }
            let file_name_matches = path
                .file_name()
                .map(|name| {
                    let name = name.to_string_lossy();
                    process_names
                        .iter()
                        .any(|expected| name.eq_ignore_ascii_case(expected))
                })
                .unwrap_or(false);
            if file_name_matches {
                pids.push(pid);
            }
        }
    }

    if let Some(port) = runtime_shutdown_port(entry) {
        pids.extend(find_listening_pids_by_port(port).into_iter().filter(|pid| {
            pid_matches_runtime_in_system(*pid, process_names, install_dir, &system)
        }));
    }

    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(windows)]
fn runtime_shutdown_port(entry: &InstalledRuntime) -> Option<u16> {
    match entry.runtime_kind.as_str() {
        "apache" => Some(80),
        "php" => entry.php_port,
        _ => None,
    }
}

#[cfg(windows)]
fn find_listening_pids_by_port(port: u16) -> Vec<u32> {
    collect_listening_tcp_pids()
        .remove(&port)
        .unwrap_or_default()
}

#[cfg(windows)]
fn parse_windows_netstat_listening_line(line: &str) -> Option<(u16, u32)> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let columns = trimmed.split_whitespace().collect::<Vec<_>>();
    if columns.len() < 4 {
        return None;
    }

    let state = columns
        .get(columns.len().saturating_sub(2))
        .copied()
        .unwrap_or_default();
    if !state.eq_ignore_ascii_case("LISTENING") {
        return None;
    }

    let local_addr = columns.get(1).copied().unwrap_or_default();
    let port = local_addr.rsplit(':').next()?.parse::<u16>().ok()?;
    let pid = columns.last()?.parse::<u32>().ok()?;
    Some((port, pid))
}

#[cfg(windows)]
fn collect_listening_tcp_pids_from_output(output: &str) -> HashMap<u16, Vec<u32>> {
    let mut listening = HashMap::<u16, Vec<u32>>::new();

    for line in output.lines() {
        if let Some((port, pid)) = parse_windows_netstat_listening_line(line) {
            listening.entry(port).or_default().push(pid);
        }
    }

    for pids in listening.values_mut() {
        pids.sort_unstable();
        pids.dedup();
    }

    listening
}

#[cfg(windows)]
fn collect_listening_tcp_pids() -> HashMap<u16, Vec<u32>> {
    let mut command = Command::new("netstat");
    let output = match hide_windows_console_window(
        command
            .args(["-ano", "-p", "TCP"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    )
    .output()
    {
        Ok(output) => output,
        Err(_) => return HashMap::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    collect_listening_tcp_pids_from_output(&stdout)
}

#[cfg(windows)]
fn apache_port_conflict_detail(entry: &InstalledRuntime) -> Option<String> {
    let inspection = RuntimeInspection::collect();
    let install_dir = Path::new(&entry.install_dir);
    let process_names = runtime_process_names(&entry.runtime_kind);
    let conflicts = apache_configured_ports(install_dir)
        .into_iter()
        .filter_map(|port| {
            let offenders = inspection
                .listening_tcp_pids
                .get(&port)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|pid| {
                    !pid_matches_runtime_in_system(
                        *pid,
                        process_names,
                        install_dir,
                        &inspection.system,
                    )
                })
                .collect::<Vec<_>>();
            if offenders.is_empty() {
                return None;
            }

            Some(format!(
                "Port {port} is already being used by {}",
                describe_processes(&offenders, &inspection.system)
            ))
        })
        .collect::<Vec<_>>();

    if conflicts.is_empty() {
        None
    } else {
        Some(conflicts.join(" | "))
    }
}

#[cfg(not(windows))]
fn apache_port_conflict_detail(_entry: &InstalledRuntime) -> Option<String> {
    None
}

fn apache_configured_ports(install_dir: &Path) -> Vec<u16> {
    let mut ports = fs::read_to_string(install_dir.join("conf").join("httpd.conf"))
        .ok()
        .map(|contents| {
            contents
                .lines()
                .filter_map(parse_apache_listen_port)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if ports.is_empty() {
        ports.push(80);
    }

    ports.sort_unstable();
    ports.dedup();
    ports
}

fn parse_apache_listen_port(line: &str) -> Option<u16> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    let mut parts = trimmed.split_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("Listen") {
        return None;
    }

    parse_apache_port_value(parts.next()?)
}

fn parse_apache_port_value(value: &str) -> Option<u16> {
    let trimmed = value.trim_matches(|ch| matches!(ch, '"' | '\''));
    trimmed
        .rsplit(':')
        .next()
        .unwrap_or(trimmed)
        .trim_matches(|ch| matches!(ch, '[' | ']'))
        .parse::<u16>()
        .ok()
}

fn describe_processes(pids: &[u32], system: &System) -> String {
    let mut labels = Vec::new();
    let mut seen = HashSet::new();
    for pid in pids {
        let label = describe_process(*pid, system);
        if seen.insert(label.to_ascii_lowercase()) {
            labels.push(label);
        }
    }

    if labels.is_empty() {
        "another process".to_string()
    } else {
        labels.join(", ")
    }
}

fn describe_process(pid: u32, system: &System) -> String {
    let name = system
        .process(sysinfo::Pid::from(pid as usize))
        .and_then(|process| {
            process
                .exe()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().to_string())
                .or_else(|| {
                    let process_name = process.name().trim().to_string();
                    (!process_name.is_empty()).then_some(process_name)
                })
        })
        .unwrap_or_else(|| "unknown process".to_string());
    format!("{name} (PID {pid})")
}

#[cfg(windows)]
#[allow(dead_code)]
fn find_listening_ports_for_runtime(process_names: &[&str], install_dir: &Path) -> Vec<u16> {
    let inspection = RuntimeInspection::collect();
    find_listening_ports_for_runtime_in_inspection(process_names, install_dir, &inspection)
}

#[cfg(windows)]
fn find_listening_ports_for_runtime_in_inspection(
    process_names: &[&str],
    install_dir: &Path,
    inspection: &RuntimeInspection,
) -> Vec<u16> {
    let mut ports = inspection
        .listening_tcp_pids
        .iter()
        .filter_map(|(port, pids)| {
            pids.iter()
                .copied()
                .any(|pid| {
                    pid_matches_runtime_in_system(
                        pid,
                        process_names,
                        install_dir,
                        &inspection.system,
                    )
                })
                .then_some(*port)
        })
        .collect::<Vec<_>>();
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn pid_matches_runtime_in_system(
    pid: u32,
    process_names: &[&str],
    install_dir: &Path,
    system: &System,
) -> bool {
    let Some(process) = system.process(sysinfo::Pid::from(pid as usize)) else {
        return false;
    };

    let Some(path) = process.exe() else {
        return false;
    };
    if !process_path_matches_install_dir(path, install_dir) {
        return false;
    }

    path.file_name()
        .map(|name| {
            let name = name.to_string_lossy();
            process_names
                .iter()
                .any(|expected| name.eq_ignore_ascii_case(expected))
        })
        .unwrap_or(false)
}

fn runtime_stop_priority(runtime_kind: &str) -> u8 {
    match runtime_kind {
        "apache" => 0,
        "php" => 1,
        "mysql" => 2,
        _ => 3,
    }
}

fn runtime_start_priority(runtime_kind: &str) -> u8 {
    match runtime_kind {
        "mysql" => 0,
        "php" => 1,
        "apache" => 2,
        _ => 3,
    }
}

fn start_all_installed_runtimes() -> Result<String, String> {
    let mut entries = load_runtime_registry()?
        .entries
        .into_iter()
        .filter(|entry| {
            entry.runtime_kind != "phpmyadmin"
                && is_runtime_entry_ready(entry)
                && entry.state != "running"
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        runtime_start_priority(&left.runtime_kind)
            .cmp(&runtime_start_priority(&right.runtime_kind))
            .then_with(|| left.version.cmp(&right.version))
    });

    if entries.is_empty() {
        return Ok("All installed runtimes are already running.".to_string());
    }

    let mut started = Vec::new();
    let mut errors = Vec::new();
    for entry in entries {
        match start_installed_runtime(&entry.id) {
            Ok(_) => started.push(format!("{} {}", entry.title, entry.version)),
            Err(error) => errors.push(format!("{} {}: {error}", entry.title, entry.version)),
        }
    }

    if errors.is_empty() {
        Ok(format!("Started {} runtime(s).", started.len()))
    } else if started.is_empty() {
        Err(format!("Failed to start runtimes: {}", errors.join("; ")))
    } else {
        Err(format!(
            "Started {} runtime(s), but some failed: {}",
            started.len(),
            errors.join("; ")
        ))
    }
}

fn stop_all_installed_runtimes() -> Result<String, String> {
    let mut entries = load_runtime_registry()?
        .entries
        .into_iter()
        .filter(|entry| {
            entry.runtime_kind != "phpmyadmin"
                && is_runtime_entry_ready(entry)
                && entry.state == "running"
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        runtime_stop_priority(&left.runtime_kind)
            .cmp(&runtime_stop_priority(&right.runtime_kind))
            .then_with(|| left.version.cmp(&right.version))
    });

    if entries.is_empty() {
        return Ok("All installed runtimes are already stopped.".to_string());
    }

    let mut stopped = Vec::new();
    let mut errors = Vec::new();
    for entry in entries {
        match stop_installed_runtime(&entry.id) {
            Ok(_) => stopped.push(format!("{} {}", entry.title, entry.version)),
            Err(error) => errors.push(format!("{} {}: {error}", entry.title, entry.version)),
        }
    }

    if errors.is_empty() {
        Ok(format!("Stopped {} runtime(s).", stopped.len()))
    } else if stopped.is_empty() {
        Err(format!("Failed to stop runtimes: {}", errors.join("; ")))
    } else {
        Err(format!(
            "Stopped {} runtime(s), but some failed: {}",
            stopped.len(),
            errors.join("; ")
        ))
    }
}

fn uninstall_installed_runtime(runtime_id: &str) -> Result<String, String> {
    stop_installed_runtime(runtime_id)?;

    let mut registry = load_runtime_registry()?;
    let index = registry
        .entries
        .iter()
        .position(|entry| entry.id == runtime_id)
        .ok_or_else(|| "Runtime is not installed".to_string())?;

    let removed = registry.entries.remove(index);
    let _ = uninstall_native_windows_runtime(&removed);
    let install_dir = PathBuf::from(&removed.install_dir);
    if install_dir.exists() {
        fs::remove_dir_all(&install_dir)
            .map_err(|error| format!("Failed to remove install directory: {error}"))?;
    }
    remove_runtime_download_artifacts(&removed, &install_dir);

    if removed.runtime_kind == "php" {
        let mut bindings = website::load_website_bindings().unwrap_or_default();
        let removed_binding_id = runtime_binding_id(&removed);
        bindings.entries.retain(|binding| {
            binding.php_runtime_id != removed_binding_id && binding.php_runtime_id != removed.id
        });
        website::save_website_bindings(&bindings)?;
        sync_apache_site_bindings(&mut registry)?;
    }

    save_runtime_registry(&registry)?;
    Ok("Runtime uninstalled".to_string())
}

fn restart_installed_runtime(runtime_id: &str) -> Result<String, String> {
    stop_installed_runtime(runtime_id)?;
    start_installed_runtime(runtime_id)?;
    Ok("Runtime restarted".to_string())
}

fn reload_installed_runtime(runtime_id: &str) -> Result<String, String> {
    stop_installed_runtime(runtime_id)?;
    start_installed_runtime(runtime_id)?;
    Ok("Runtime reloaded".to_string())
}

fn open_installed_runtime_path(runtime_id: &str) -> Result<String, String> {
    let registry = load_runtime_registry()?;
    let entry = registry
        .entries
        .iter()
        .find(|entry| entry.id == runtime_id)
        .ok_or_else(|| "Runtime is not installed".to_string())?;
    let install_dir = PathBuf::from(&entry.install_dir);
    if !install_dir.exists() {
        return Err(format!(
            "Install path does not exist: {}",
            install_dir.display()
        ));
    }
    if !install_dir.is_dir() {
        return Err(format!(
            "Install path is not a directory: {}",
            install_dir.display()
        ));
    }

    open_path_in_file_manager(&install_dir)?;
    Ok(format!("Opened {}", install_dir.display()))
}

fn open_path_in_file_manager(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let mut command = Command::new("explorer");
        command.arg(path);
        command.creation_flags(CREATE_NO_WINDOW);
        return command
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Failed to open path in Explorer: {error}"));
    }

    #[cfg(target_os = "macos")]
    {
        return Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Failed to open path in Finder: {error}"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Failed to open path in file manager: {error}"))
    }
}

fn remove_runtime_download_artifacts(entry: &InstalledRuntime, install_dir: &Path) {
    let downloads_root = resolve_data_base_dir().map(|base| base.join("data").join("downloads"));
    if let Some(root) = downloads_root {
        for candidate in [
            Some(PathBuf::from(&entry.package_file)),
            Some(root.join("plugins").join(format!(
                "{}-{}.zip",
                sanitize_path_segment(&entry.name),
                sanitize_path_segment(&entry.version)
            ))),
            Some(
                root.join("scripts")
                    .join(format!("{}.bat", sanitize_path_segment(&entry.name))),
            ),
        ]
        .into_iter()
        .flatten()
        {
            if candidate.starts_with(install_dir) {
                continue;
            }
            if !candidate.starts_with(&root) {
                continue;
            }
            remove_file_if_exists(&candidate);
        }
    }
}

fn remove_file_if_exists(path: &Path) {
    if !path.exists() {
        return;
    }
    if fs::remove_file(path).is_ok() {
        return;
    }

    #[cfg(windows)]
    {
        if let Ok(metadata) = fs::metadata(path) {
            let mut permissions = metadata.permissions();
            if permissions.readonly() {
                permissions.set_readonly(false);
                let _ = fs::set_permissions(path, permissions);
                let _ = fs::remove_file(path);
            }
        }
    }
}

pub(crate) fn sync_apache_site_bindings(registry: &mut RuntimeRegistry) -> Result<(), String> {
    let Some(apache_index) = select_primary_runtime_index(registry, "apache") else {
        return Ok(());
    };

    let bindings = website::load_website_bindings().unwrap_or_default();
    let apache_entry = registry.entries[apache_index].clone();
    let php_registry = registry
        .entries
        .iter()
        .filter(|entry| entry.runtime_kind == "php" && is_runtime_entry_ready(entry))
        .map(|entry| (runtime_binding_id(entry), entry.clone()))
        .collect::<HashMap<_, _>>();
    let data_root = resolve_data_base_dir()
        .ok_or_else(|| "Unable to resolve application directory".to_string())?
        .join("data");
    let sites = website::collect_website_sources()
        .into_iter()
        .map(|source| {
            let binding = bindings
                .entries
                .iter()
                .find(|binding| binding.site_id == source.id);
            let php_binding_id = binding
                .map(|binding| binding.php_runtime_id.clone())
                .and_then(|binding_id| resolve_php_runtime_binding_id(&binding_id, &php_registry));
            let php_port = php_binding_id
                .as_deref()
                .and_then(|binding_id| php_registry.get(binding_id))
                .and_then(|runtime| runtime.php_port);
            let ssl = website::ssl_paths_for_domain(&source.domain).map(|(cert, key)| {
                json!({
                    "cert": cert.display().to_string(),
                    "key": key.display().to_string(),
                })
            });

            json!({
                "id": source.id,
                "domain": source.domain,
                "path": source.path.display().to_string(),
                "enabled": binding.map(|binding| binding.enabled).unwrap_or(true),
                "php_port": php_port,
                "ssl": ssl,
            })
        })
        .collect::<Vec<_>>();
    LuaPluginEngine::new().call_hook_json(
        "apache",
        "sync_sites",
        &json!({
            "install_dir": apache_entry.install_dir,
            "website_root": website::resolve_website_root().display().to_string(),
            "data_root": data_root.display().to_string(),
            "sites": sites,
            "phpmyadmin_dir": select_primary_runtime(registry, "phpmyadmin").map(|entry| entry.install_dir.clone()),
            "phpmyadmin_php_port": select_primary_runtime(registry, "php").and_then(|entry| entry.php_port),
        }),
    )?;

    restart_apache_runtime_if_running(&mut registry.entries[apache_index])
}

fn select_primary_runtime_index(registry: &RuntimeRegistry, runtime_kind: &str) -> Option<usize> {
    registry
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.runtime_kind == runtime_kind && is_runtime_entry_ready(entry))
        .max_by(|(_, left), (_, right)| {
            let left_running = detect_runtime_pid(left).is_some() || left.state == "running";
            let right_running = detect_runtime_pid(right).is_some() || right.state == "running";
            left_running
                .cmp(&right_running)
                .then_with(|| left.version.cmp(&right.version))
        })
        .map(|(index, _)| index)
}

fn select_primary_runtime<'a>(
    registry: &'a RuntimeRegistry,
    runtime_kind: &str,
) -> Option<&'a InstalledRuntime> {
    select_primary_runtime_index(registry, runtime_kind).map(|index| &registry.entries[index])
}

fn restart_apache_runtime_if_running(entry: &mut InstalledRuntime) -> Result<(), String> {
    entry.pid = detect_runtime_pid(entry);
    if entry.pid.is_none() && entry.state != "running" {
        entry.state = runtime_default_state(&entry.runtime_kind);
        return Ok(());
    }

    let install_dir = Path::new(&entry.install_dir);
    if let Some(result) =
        run_native_windows_runtime_action("stop", install_dir, &entry.runtime_kind, entry.php_port)
    {
        result?;
    }
    thread::sleep(Duration::from_millis(500));
    cleanup_runtime_pid_file(install_dir, &entry.runtime_kind);
    entry.pid = None;
    entry.state = runtime_default_state(&entry.runtime_kind);

    ensure_runtime_start_preconditions(entry)?;
    if let Some(result) =
        run_native_windows_runtime_action("start", install_dir, &entry.runtime_kind, entry.php_port)
    {
        result?;
    }
    if !wait_for_runtime_start(entry, Duration::from_secs(10)) {
        let detail = runtime_start_failure_detail(entry)
            .map(|detail| format!(": {detail}"))
            .unwrap_or_default();
        return Err(format!(
            "Apache restart completed but the runtime was not detected{detail}"
        ));
    }
    Ok(())
}

fn detect_runtime_executable(
    install_root: &Path,
    runtime_kind: &str,
    recursive: bool,
) -> Option<PathBuf> {
    for candidate in preferred_runtime_executable_candidates(install_root, runtime_kind) {
        if candidate.exists() {
            return Some(candidate);
        }
    }

    let names = match runtime_kind {
        "apache" => &["httpd.exe"][..],
        "mysql" => &["mysqld.exe"][..],
        "php" => &["php-cgi.exe", "php.exe"][..],
        _ => &[][..],
    };
    if recursive {
        for name in names {
            if let Some(path) = find_file_recursive(install_root, name) {
                return Some(path);
            }
        }
    }
    None
}

fn preferred_runtime_executable_candidates(
    install_root: &Path,
    runtime_kind: &str,
) -> Vec<PathBuf> {
    match runtime_kind {
        "apache" => vec![
            install_root.join("bin").join("httpd.exe"),
            install_root.join("Apache24").join("bin").join("httpd.exe"),
        ],
        "mysql" => vec![
            install_root.join("bin").join("mysqld.exe"),
            install_root.join("mysqld.exe"),
        ],
        "php" => vec![
            install_root.join("php-cgi.exe"),
            install_root.join("php.exe"),
            install_root.join("bin").join("php-cgi.exe"),
            install_root.join("bin").join("php.exe"),
        ],
        _ => Vec::new(),
    }
}

fn cleanup_runtime_pid_file(install_root: &Path, runtime_kind: &str) {
    let pid_path = runtime_pid_file(install_root, runtime_kind);
    if pid_path.as_os_str().is_empty() {
        return;
    }
    if !pid_path.exists() {
        return;
    }
    let pid = fs::read_to_string(&pid_path)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    if pid.map(is_process_running).unwrap_or(false) {
        return;
    }
    let _ = fs::remove_file(pid_path);
}

fn read_runtime_pid_value(install_root: &Path, runtime_kind: &str) -> Option<u32> {
    let pid_path = runtime_pid_file(install_root, runtime_kind);
    if pid_path.as_os_str().is_empty() {
        return None;
    }

    fs::read_to_string(pid_path)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
}

fn apache_pid_file(install_root: &Path) -> PathBuf {
    install_root.join("logs").join("httpd.pid")
}

fn php_pid_file(install_root: &Path) -> PathBuf {
    install_root.join("logs").join("php-cgi.pid")
}

fn runtime_pid_file(install_root: &Path, runtime_kind: &str) -> PathBuf {
    match runtime_kind {
        "apache" => apache_pid_file(install_root),
        "php" => php_pid_file(install_root),
        _ => PathBuf::new(),
    }
}

pub(crate) fn is_runtime_entry_ready(entry: &InstalledRuntime) -> bool {
    let install_root = Path::new(&entry.install_dir);
    if !install_root.exists() {
        return false;
    }
    if entry.runtime_kind == "phpmyadmin" {
        return true;
    }
    if entry
        .executable_path
        .as_ref()
        .map(|path| Path::new(path).exists())
        .unwrap_or(false)
    {
        return true;
    }
    detect_runtime_executable(install_root, &entry.runtime_kind, true).is_some()
}

fn find_file_recursive(root: &Path, file_name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.eq_ignore_ascii_case(file_name))
                .unwrap_or(false)
        {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, file_name) {
                return Some(found);
            }
        }
    }
    None
}

fn upsert_runtime_entry(registry: &mut RuntimeRegistry, runtime: InstalledRuntime) {
    if let Some(existing) = registry
        .entries
        .iter_mut()
        .find(|entry| entry.id == runtime.id)
    {
        *existing = runtime;
    } else {
        registry.entries.push(runtime);
    }
}

fn php_fastcgi_port(version: &str) -> u16 {
    let parts = version.split('.').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 3 {
        return 18900;
    }

    let major = parts[0].parse::<u16>().unwrap_or_default();
    let minor = parts[1].parse::<u16>().unwrap_or_default();
    let patch = parts[2].parse::<u16>().unwrap_or_default();
    9000u16
        .saturating_add(major.saturating_mul(1000))
        .saturating_add(minor.saturating_mul(100))
        .saturating_add(patch)
}

fn resolve_php_runtime_port(
    install_dir: &Path,
    version: &str,
    inspection: &RuntimeInspection,
) -> u16 {
    detect_live_php_runtime_port(install_dir, inspection)
        .unwrap_or_else(|| php_fastcgi_port(version))
}

#[cfg(windows)]
fn detect_live_php_runtime_port(install_dir: &Path, inspection: &RuntimeInspection) -> Option<u16> {
    find_listening_ports_for_runtime_in_inspection(
        &["php-cgi.exe", "php.exe"],
        install_dir,
        inspection,
    )
    .into_iter()
    .min()
}

#[cfg(not(windows))]
fn detect_live_php_runtime_port(
    _install_dir: &Path,
    _inspection: &RuntimeInspection,
) -> Option<u16> {
    None
}

fn cleanup_legacy_runtime_metadata(install_dir: &Path) {
    let path = install_dir.join("MinPanel-runtime.json");
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}

pub(crate) fn sanitize_path_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            _ => ch,
        })
        .collect::<String>()
}

fn is_process_running(pid: u32) -> bool {
    let system = collect_process_system();
    is_process_running_in_system(pid, &system)
}

fn collect_process_system() -> System {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessRefreshKind::everything());
    system
}

fn is_process_running_in_system(pid: u32, system: &System) -> bool {
    system.process(sysinfo::Pid::from(pid as usize)).is_some()
}

async fn download_software_store() -> Result<String, String> {
    let data_base_dir = resolve_data_base_dir()
        .ok_or_else(|| "Unable to resolve application directory".to_string())?;
    let local_path = data_base_dir.join("data").join("plugin.json");

    if local_path.exists() {
        return fs::read_to_string(local_path)
            .map_err(|error| format!("Failed to read local plugin list: {error}"));
    }

    // Fallback or error if local file is missing
    Err("Local plugin.json not found in data directory. Please ensure it exists.".to_string())
}

fn validate_plugin_store(contents: &str) -> Result<(), String> {
    serde_json::from_str::<PluginStoreFile>(contents)
        .map(|_| ())
        .map_err(|error| format!("Invalid plugin.json payload: {error}"))
}

fn map_plugin_entry(
    plugin: PluginRaw,
    type_map: &HashMap<i64, String>,
    _base_dir: &Path,
    registry: &RuntimeRegistry,
) -> SoftwarePluginEntry {
    let category = type_map
        .get(&plugin.r#type)
        .cloned()
        .unwrap_or_else(|| "Other".to_string());
    let price = parse_price(&plugin.price);
    let version = select_plugin_version(&plugin);
    let runtime_kind = detect_runtime_kind(&plugin.name, &plugin.dependent);
    let id = build_runtime_id(&plugin.name, &version, &runtime_kind);
    let registry_entry = registry
        .entries
        .iter()
        .find(|entry| entry.id == id)
        .filter(|entry| is_runtime_entry_ready(entry));
    let installed = registry_entry.is_some();
    let expire = format_plugin_expire(plugin.endtime, installed);
    let status = registry_entry
        .map(|entry| entry.state.clone())
        .unwrap_or_else(|| {
            if installed {
                "stopped".to_string()
            } else {
                "stopped".to_string()
            }
        });
    let actions = build_plugin_actions(
        installed,
        price,
        &status,
        detect_runtime_kind(&plugin.name, &plugin.dependent),
    );
    let title = plugin.title.clone();
    let description = simplify_plugin_description(&plugin.ps);
    let visual = infer_plugin_visual(&plugin.name, &plugin.dependent);

    SoftwarePluginEntry {
        id,
        name: plugin.name,
        title,
        version,
        developer: "official".to_string(),
        description,
        price,
        expire,
        category,
        installed,
        status,
        path: if installed {
            "Open".to_string()
        } else {
            "--".to_string()
        },
        actions,
        visual,
    }
}

fn map_installed_runtime_entry(
    entry: &InstalledRuntime,
    plugin: Option<&PluginRaw>,
    type_map: &HashMap<i64, String>,
) -> SoftwarePluginEntry {
    let category = plugin
        .and_then(|plugin| type_map.get(&plugin.r#type))
        .cloned()
        .unwrap_or_else(|| "Runtime".to_string());
    let title = plugin
        .map(|plugin| plugin.title.clone())
        .unwrap_or_else(|| entry.title.clone());
    let base_description = plugin
        .map(|plugin| simplify_plugin_description(&plugin.ps))
        .unwrap_or_else(|| format!("Installed {} runtime.", title));
    let description = if entry.runtime_kind == "php" {
        entry
            .php_port
            .map(|port| format!("{base_description} FastCGI port {port}."))
            .unwrap_or(base_description)
    } else {
        base_description
    };

    SoftwarePluginEntry {
        id: entry.id.clone(),
        name: entry.name.clone(),
        title,
        version: entry.version.clone(),
        developer: "official".to_string(),
        description,
        price: 0.0,
        expire: "Permanent".to_string(),
        category,
        installed: true,
        status: entry.state.clone(),
        path: "Open".to_string(),
        actions: vec!["Uninstall".to_string()],
        visual: infer_plugin_visual(&entry.name, &entry.runtime_kind),
    }
}

fn parse_price(value: &serde_json::Value) -> f64 {
    match value {
        serde_json::Value::Number(number) => number.as_f64().unwrap_or(0.0),
        serde_json::Value::String(text) => text.parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn select_plugin_version(plugin: &PluginRaw) -> String {
    if !plugin.version.is_empty() && plugin.version != "0" {
        return plugin.version.clone();
    }
    if let Some(version) = effective_plugin_versions(plugin).first() {
        return select_plugin_version_entry(version, &plugin.version);
    }
    "--".to_string()
}

fn format_plugin_expire(endtime: i64, installed: bool) -> String {
    match endtime {
        0 => "Permanent".to_string(),
        -1 => "Not open".to_string(),
        -2 => "Already expire".to_string(),
        value if value > 0 => "Authorized".to_string(),
        _ if installed => "Permanent".to_string(),
        _ => "--".to_string(),
    }
}

fn build_plugin_actions(
    installed: bool,
    price: f64,
    status: &str,
    runtime_kind: String,
) -> Vec<String> {
    if installed {
        let _ = status;
        let _ = runtime_kind;
        return vec!["Uninstall".to_string()];
    }
    if price > 0.0 {
        return vec!["Buy now".to_string()];
    }
    vec!["Install".to_string()]
}

fn simplify_plugin_description(input: &str) -> String {
    let mut plain = String::with_capacity(input.len());
    let mut in_tag = false;
    let mut previous_space = false;

    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            '&' => {}
            '\n' | '\r' | '\t' => {
                if !previous_space {
                    plain.push(' ');
                    previous_space = true;
                }
            }
            _ if ch.is_whitespace() => {
                if !previous_space {
                    plain.push(' ');
                    previous_space = true;
                }
            }
            _ => {
                plain.push(ch);
                previous_space = false;
            }
        }
    }

    let cleaned = plain
        .replace("nbsp;", " ")
        .replace("&gt;&gt;", "")
        .replace("&gt;", "")
        .replace("&lt;", "")
        .replace("&amp;", "&");
    let compact = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return "Plugin from App Store".to_string();
    }
    if compact.chars().count() <= 140 {
        return compact;
    }
    let shortened = compact.chars().take(137).collect::<String>();
    format!("{}...", shortened.trim_end())
}

fn infer_plugin_visual(name: &str, dependent: &str) -> String {
    let lower_name = name.to_ascii_lowercase();
    let lower_dependent = dependent.to_ascii_lowercase();
    let haystack = format!("{lower_name}|{lower_dependent}");

    if haystack.contains("apache") {
        return "apache".to_string();
    }
    if haystack.contains("nginx") {
        return "nginx".to_string();
    }
    if haystack.contains("mysql") {
        return "dolphin".to_string();
    }
    if haystack.contains("php") {
        return "php".to_string();
    }
    if haystack.contains("node") || haystack.contains("pm2") {
        return "node".to_string();
    }
    if haystack.contains("redis") {
        return "redis".to_string();
    }
    if haystack.contains("memcached") {
        return "memcached".to_string();
    }
    if haystack.contains("waf") || haystack.contains("tamper") || haystack.contains("security") {
        return "waf".to_string();
    }
    if haystack.contains("ftp") || haystack.contains("file") || haystack.contains("s3") {
        return "lock".to_string();
    }
    "target".to_string()
}

fn detect_runtime_kind(name: &str, dependent: &str) -> String {
    let lower_name = name.to_ascii_lowercase();
    let lower_dependent = dependent.to_ascii_lowercase();
    let haystack = format!("{lower_name}|{lower_dependent}");
    if lower_name == "phpmyadmin" {
        return "phpmyadmin".to_string();
    }
    if haystack.contains("apache") {
        return "apache".to_string();
    }
    if haystack.contains("mysql") {
        return "mysql".to_string();
    }
    if lower_name.starts_with("php-") || haystack.contains("php") {
        return "php".to_string();
    }
    if haystack.contains("nginx") {
        return "nginx".to_string();
    }
    "generic".to_string()
}

pub(crate) fn resolve_data_base_dir() -> Option<PathBuf> {
    let base = if let Ok(executable) = env::current_exe() {
        if let Some(parent) = executable.parent() {
            parent.to_path_buf()
        } else {
            env::current_dir().ok()?
        }
    } else {
        env::current_dir().ok()?
    };

    // Only log once to avoid flooding console during polling
    static LOGGED: OnceLock<()> = OnceLock::new();
    LOGGED.get_or_init(|| {
        println!(
            "[System] Data base directory resolved to: {}",
            base.display()
        );
    });

    Some(base)
}

pub(crate) fn resolve_env_path_override(var_name: &str) -> Option<PathBuf> {
    let raw = env::var(var_name).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let override_path = PathBuf::from(trimmed);
    if override_path.is_absolute() {
        return Some(override_path);
    }

    resolve_data_base_dir()
        .map(|base_dir| base_dir.join(&override_path))
        .or_else(|| {
            env::current_dir()
                .ok()
                .map(|current_dir| current_dir.join(&override_path))
        })
        .or(Some(override_path))
}

fn resolve_resource_base_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(executable) = env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    if let Ok(current_dir) = env::current_dir() {
        candidates.push(current_dir);
    }

    for candidate in &candidates {
        if let Some(root) = find_workspace_root(candidate) {
            return Some(root);
        }
    }

    candidates.into_iter().next()
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(path) = current {
        if path.join("Cargo.toml").exists() {
            return Some(path.to_path_buf());
        }
        current = path.parent();
    }
    None
}

pub(crate) fn slugify(input: &str, separator: char) -> String {
    let mut value = String::new();
    let mut previous_was_separator = false;

    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            value.push(ch.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator && !value.is_empty() {
            value.push(separator);
            previous_was_separator = true;
        }
    }

    value.trim_matches(separator).to_string()
}

fn collect_disks() -> Vec<DiskData> {
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .map(|disk| DiskData {
            name: disk.name().to_string_lossy().to_string(),
            mount_point: disk.mount_point().display().to_string(),
            total_space: disk.total_space(),
            available_space: disk.available_space(),
        })
        .collect()
}

fn find_app_disk(disks: &[DiskData], workspace_root: &str) -> Option<DiskData> {
    let workspace = workspace_root.to_ascii_lowercase();

    disks
        .iter()
        .filter(|disk| {
            let mount = disk.mount_point.to_ascii_lowercase();
            workspace.starts_with(&mount)
        })
        .max_by_key(|disk| disk.mount_point.len())
        .cloned()
        .or_else(|| disks.first().cloned())
}

fn collect_networks() -> Vec<NetworkData> {
    let networks = Networks::new_with_refreshed_list();
    let mut list = networks
        .list()
        .iter()
        .map(|(name, data)| NetworkData {
            name: name.to_string(),
            received: data.received(),
            transmitted: data.transmitted(),
            total_received: data.total_received(),
            total_transmitted: data.total_transmitted(),
        })
        .collect::<Vec<_>>();

    list.sort_by(|left, right| {
        let left_total = left.total_received + left.total_transmitted;
        let right_total = right.total_received + right.total_transmitted;
        right_total.cmp(&left_total)
    });

    list
}

fn collect_processes(system: &System) -> Vec<ProcessData> {
    let mut processes = system
        .processes()
        .values()
        .map(|process| ProcessData {
            pid: process.pid().as_u32(),
            name: process.name().to_string(),
            cpu_usage: process.cpu_usage(),
            memory: process.memory(),
            status: format!("{:?}", process.status()),
        })
        .collect::<Vec<_>>();

    processes.sort_by(|left, right| {
        right
            .cpu_usage
            .partial_cmp(&left.cpu_usage)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.memory.cmp(&left.memory))
    });
    processes.truncate(8);
    processes
}

fn build_alerts(
    cpu_usage: f32,
    used_memory: u64,
    total_memory: u64,
    disks: &[DiskData],
) -> Vec<String> {
    let mut alerts = Vec::new();
    let memory_usage = if total_memory == 0 {
        0.0
    } else {
        used_memory as f64 / total_memory as f64 * 100.0
    };

    if cpu_usage >= 85.0 {
        alerts.push(format!("CPU usage is elevated at {:.1}%.", cpu_usage));
    }
    if memory_usage >= 85.0 {
        alerts.push(format!("Memory usage is elevated at {:.1}%.", memory_usage));
    }

    for disk in disks {
        let used_percent = if disk.total_space == 0 {
            0.0
        } else {
            (disk.total_space - disk.available_space) as f64 / disk.total_space as f64 * 100.0
        };

        if used_percent >= 90.0 {
            alerts.push(format!(
                "Disk {} is close to full at {:.1}% usage.",
                disk.mount_point, used_percent
            ));
        }
    }

    alerts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_plugin_variants_splits_multi_version_manifest() {
        let variants = expand_plugin_variants(PluginRaw {
            name: "php".to_string(),
            title: "PHP".to_string(),
            version: String::new(),
            ps: "PHP runtime".to_string(),
            price: serde_json::Value::String("0".to_string()),
            endtime: 0,
            r#type: 1,
            sort: 2,
            dependent: "php".to_string(),
            install_checks: String::new(),
            versions: vec![
                PluginVersionRaw {
                    version: "8.3.30".to_string(),
                    full_version: String::new(),
                    f_path: String::new(),
                    install_checks: "data/plugins/php-8.3".to_string(),
                    downloads: vec!["https://example.com/php-8.3.30.zip".to_string()],
                    mirrors: vec!["https://mirror.example.com/php-8.3.30.zip".to_string()],
                },
                PluginVersionRaw {
                    version: "8.4.20".to_string(),
                    full_version: String::new(),
                    f_path: String::new(),
                    install_checks: "data/plugins/php-8.4".to_string(),
                    downloads: vec!["https://example.com/php-8.4.20.zip".to_string()],
                    mirrors: Vec::new(),
                },
            ],
        });

        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].version, "8.3.30");
        assert_eq!(variants[0].install_checks, "data/plugins/php-8.3");
        assert_eq!(variants[1].version, "8.4.20");
        assert_eq!(variants[1].install_checks, "data/plugins/php-8.4");
        assert_eq!(variants[1].versions.len(), 1);
    }

    #[test]
    fn plugin_download_urls_include_primary_and_mirrors_without_duplicates() {
        let urls = plugin_download_urls(&PluginVersionRaw {
            version: "8.3.30".to_string(),
            full_version: "8.3.30".to_string(),
            f_path: "https://example.com/php-8.3.30.zip".to_string(),
            install_checks: String::new(),
            downloads: vec![
                "https://example.com/php-8.3.30.zip".to_string(),
                "https://cdn.example.com/php-8.3.30.zip".to_string(),
            ],
            mirrors: vec!["https://mirror.example.com/php-8.3.30.zip".to_string()],
        });

        assert_eq!(
            urls,
            vec![
                "https://example.com/php-8.3.30.zip".to_string(),
                "https://cdn.example.com/php-8.3.30.zip".to_string(),
                "https://mirror.example.com/php-8.3.30.zip".to_string(),
            ]
        );
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let path = env::temp_dir().join(format!("MinPanel-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).expect("failed to create temp test directory");
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn test_web_runtime(root: &Path, kind: &str, version: &str, state: &str) -> InstalledRuntime {
        let install_dir = root.join(kind).join(version);
        fs::create_dir_all(&install_dir).expect("failed to create runtime install dir");
        let executable_path = install_dir.join(format!("{kind}.exe"));
        fs::write(&executable_path, "").expect("failed to write runtime executable");

        InstalledRuntime {
            id: format!("{kind}-{version}"),
            name: kind.to_string(),
            title: web_server_runtime_label(kind),
            version: version.to_string(),
            runtime_kind: kind.to_string(),
            install_dir: install_dir.display().to_string(),
            package_file: String::new(),
            executable_path: Some(executable_path.display().to_string()),
            state: state.to_string(),
            pid: None,
            php_port: None,
        }
    }

    #[test]
    fn flatten_extracted_runtime_root_promotes_nested_apache_layout() {
        let temp = TestDir::new();
        let install_root = temp.path.join("runtime");
        let nested_root = install_root.join("bundle").join("Apache24");

        fs::create_dir_all(nested_root.join("conf"))
            .expect("failed to create apache conf directory");
        fs::create_dir_all(nested_root.join("bin")).expect("failed to create apache bin directory");
        fs::write(nested_root.join("conf").join("httpd.conf"), "ServerRoot .")
            .expect("failed to write apache config");
        fs::write(nested_root.join("bin").join("httpd.exe"), "")
            .expect("failed to write apache executable");

        flatten_extracted_runtime_root(&install_root, "apache")
            .expect("failed to flatten apache runtime");

        assert!(install_root.join("conf").join("httpd.conf").exists());
        assert!(install_root.join("bin").join("httpd.exe").exists());
        assert!(!install_root.join("bundle").exists());
    }

    #[test]
    fn runtime_binding_id_includes_php_version() {
        let entry = InstalledRuntime {
            id: "php".to_string(),
            name: "php".to_string(),
            title: "PHP".to_string(),
            version: "8.3.28".to_string(),
            runtime_kind: "php".to_string(),
            install_dir: String::new(),
            package_file: String::new(),
            executable_path: None,
            state: "stopped".to_string(),
            pid: None,
            php_port: Some(9000),
        };

        assert_eq!(runtime_binding_id(&entry), "php-8-3-28-php");
    }

    #[test]
    fn active_web_server_runtime_keeps_installed_stopped_runtime() {
        let temp = TestDir::new();
        let registry = RuntimeRegistry {
            entries: vec![test_web_runtime(&temp.path, "apache", "2.4.57", "stopped")],
        };

        let web_server = active_web_server_runtime(&registry).unwrap();

        assert_eq!(web_server.kind, "apache");
        assert_eq!(web_server.status, "stopped");
    }

    #[test]
    fn active_web_server_runtime_prefers_running_runtime() {
        let temp = TestDir::new();
        let registry = RuntimeRegistry {
            entries: vec![
                test_web_runtime(&temp.path, "apache", "2.4.57", "stopped"),
                test_web_runtime(&temp.path, "nginx", "1.26.0", "running"),
            ],
        };

        let web_server = active_web_server_runtime(&registry).unwrap();

        assert_eq!(web_server.kind, "nginx");
        assert_eq!(web_server.status, "running");
    }

    #[test]
    fn paths_match_for_process_distinguishes_php_versions() {
        let runtime_root = std::env::temp_dir().join("minpanel-runtime-test");
        let php_8419 = runtime_root.join("php").join("8.4.19");
        let php_8330 = runtime_root.join("php").join("8.3.30");
        let php_8419_cgi = php_8419.join("php-cgi.exe");
        let php_8330_cgi = php_8330.join("php-cgi.exe");

        assert!(paths_match_for_process(
            php_8419_cgi.as_path(),
            php_8419_cgi.as_path()
        ));
        assert!(!paths_match_for_process(
            php_8330_cgi.as_path(),
            php_8419_cgi.as_path()
        ));
        assert!(process_path_matches_install_dir(
            php_8419_cgi.as_path(),
            php_8419.as_path()
        ));
        assert!(!process_path_matches_install_dir(
            php_8330_cgi.as_path(),
            php_8419.as_path()
        ));
    }

    #[test]
    fn php_fastcgi_port_matches_minpanel_style_mapping() {
        assert_eq!(php_fastcgi_port("8.3.30"), 17330);
        assert_eq!(php_fastcgi_port("8.4.19"), 17419);
        assert_eq!(php_fastcgi_port("invalid"), 18900);
    }

    #[test]
    fn runtime_registry_cache_signature_tracks_runtime_state_changes() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let install_dir = std::env::temp_dir().join(format!(
            "MinPanel-dashboard-runtime-cache-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(install_dir.join("php-cgi.exe"), "").unwrap();

        let mut registry = RuntimeRegistry {
            entries: vec![InstalledRuntime {
                id: "php-8-4-19".to_string(),
                name: "php".to_string(),
                title: "PHP".to_string(),
                version: "8.4.19".to_string(),
                runtime_kind: "php".to_string(),
                install_dir: install_dir.display().to_string(),
                package_file: String::new(),
                executable_path: None,
                state: "stopped".to_string(),
                pid: None,
                php_port: Some(17419),
            }],
        };
        let stopped = runtime_registry_cache_signature(&registry);

        registry.entries[0].state = "running".to_string();
        registry.entries[0].pid = Some(17419);
        let running = runtime_registry_cache_signature(&registry);

        assert_ne!(stopped, running);

        let _ = fs::remove_dir_all(install_dir);
    }

    #[test]
    fn parse_apache_listen_port_supports_common_listen_formats() {
        assert_eq!(parse_apache_listen_port("Listen 80"), Some(80));
        assert_eq!(parse_apache_listen_port("Listen 0.0.0.0:443"), Some(443));
        assert_eq!(parse_apache_listen_port("Listen [::]:8080"), Some(8080));
        assert_eq!(parse_apache_listen_port("# Listen 80"), None);
        assert_eq!(parse_apache_listen_port("ServerName localhost:80"), None);
    }

    #[cfg(windows)]
    #[test]
    fn parse_windows_netstat_listening_line_supports_ipv4_and_ipv6() {
        assert_eq!(
            parse_windows_netstat_listening_line(
                "  TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       5356"
            ),
            Some((80, 5356))
        );
        assert_eq!(
            parse_windows_netstat_listening_line(
                "  TCP    [::]:443               [::]:0                 LISTENING       912"
            ),
            Some((443, 912))
        );
        assert_eq!(
            parse_windows_netstat_listening_line(
                "  TCP    127.0.0.1:17419        0.0.0.0:0              Established     5356"
            ),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn collect_listening_tcp_pids_from_output_groups_and_deduplicates() {
        let output = "\
Active Connections\r\n\
\r\n\
  Proto  Local Address          Foreign Address        State           PID\r\n\
  TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       5356\r\n\
  TCP    127.0.0.1:80           0.0.0.0:0              LISTENING       5356\r\n\
  TCP    [::]:80                [::]:0                 LISTENING       5356\r\n\
  TCP    127.0.0.1:17419        0.0.0.0:0              LISTENING       8124\r\n";

        let listening = collect_listening_tcp_pids_from_output(output);

        assert_eq!(listening.get(&80), Some(&vec![5356]));
        assert_eq!(listening.get(&17419), Some(&vec![8124]));
    }

    #[test]
    fn join_unique_details_deduplicates_failure_hints() {
        let detail = join_unique_details(vec![
            Some("Port 80 is already being used by nginx.exe (PID 1234)".to_string()),
            Some("port 80 is already being used by nginx.exe (PID 1234)".to_string()),
            Some("AH00072: make_sock: could not bind to address 0.0.0.0:80".to_string()),
        ])
        .expect("expected combined detail");

        assert_eq!(
            detail,
            "Port 80 is already being used by nginx.exe (PID 1234) | AH00072: make_sock: could not bind to address 0.0.0.0:80"
        );
    }
}

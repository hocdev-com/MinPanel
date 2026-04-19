use axum::{response::Html, Json};
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    collections::{HashMap, HashSet},
    env, fs,
    hash::{Hash, Hasher},
    io::{self, BufRead},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, UNIX_EPOCH},
};

use crate::dashboard::{self, OperationStatus, RuntimeRegistry};

const DEFAULT_LOCAL_DOMAIN_SUFFIX: &str = ".test";
#[cfg(windows)]
const WINDOWS_HOSTS_FILE: &str = r"C:\Windows\System32\drivers\etc\hosts";
#[cfg(windows)]
const WINDOWS_HOSTS_UPDATE_SCRIPT: &str = r"data\bin\update-hosts.bat";
#[cfg(windows)]
const WINDOWS_CMD_EXECUTABLE: &str = r"C:\Windows\System32\cmd.exe";
#[cfg(windows)]
const WINDOWS_CREATE_NO_WINDOW: u32 = 0x08000000;
static WEBSITE_ROUTING_SYNC_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static WEBSITE_SOURCE_CACHE: OnceLock<Mutex<Option<CachedWebsiteSources>>> = OnceLock::new();

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use std::ptr::{null, null_mut};
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::ShellExecuteW;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

#[derive(Serialize)]
pub struct WebsiteEntry {
    id: String,
    name: String,
    alias: String,
    category: String,
    status: String,
    backup_total: usize,
    backup_label: String,
    runtime: String,
    expiration: String,
    ssl_status: String,
    ssl_enabled: bool,
    requests: u64,
    waf: String,
    php_binding: Option<String>,
}

#[derive(Deserialize)]
pub struct WebsitePhpBindingRequest {
    site_id: String,
    php_runtime_id: String,
}

#[derive(Deserialize)]
pub struct WebsiteCreateRequest {
    domain: String,
    #[serde(default)]
    description: String,
    website_path: String,
    #[serde(default)]
    php_runtime_id: String,
    #[serde(default = "default_create_index_php")]
    create_html: bool,
    #[serde(default)]
    apply_ssl: bool,
}

#[derive(Deserialize)]
pub struct WebsiteDeleteRequest {
    site_id: String,
    #[serde(default)]
    delete_document_root: bool,
}

#[derive(Deserialize)]
pub struct WebsiteLifecycleRequest {
    site_id: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct WebsiteBindingStore {
    #[serde(default)]
    pub(crate) entries: Vec<WebsitePhpBinding>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct WebsitePhpBinding {
    pub(crate) site_id: String,
    #[serde(default)]
    pub(crate) php_runtime_id: String,
    #[serde(default)]
    pub(crate) domain: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) site_path: String,
    #[serde(default)]
    pub(crate) ssl: bool,
    #[serde(default = "default_site_enabled")]
    pub(crate) enabled: bool,
}

#[derive(Clone)]
pub(crate) struct WebsiteScan {
    pub(crate) item_count: usize,
    pub(crate) backup_files: usize,
    pub(crate) has_php: bool,
    pub(crate) has_node: bool,
    pub(crate) has_python: bool,
    pub(crate) has_proxy: bool,
}

#[derive(Clone)]
pub(crate) struct WebsiteSource {
    pub(crate) id: String,
    pub(crate) domain: String,
    pub(crate) alias: String,
    pub(crate) category: String,
    pub(crate) path: PathBuf,
    pub(crate) scan: WebsiteScan,
}

#[derive(Clone)]
struct CachedWebsiteSources {
    key: u64,
    sources: Vec<WebsiteSource>,
}

struct SiteSslState {
    expiration: String,
    status: String,
    https_available: bool,
}

struct SiteTrafficState {
    requests: u64,
}

pub async fn website_page() -> Html<String> {
    let page = include_str!("ui/dashboard/layout.html")
        .replace("{{TITLE}}", "MinPanel Website")
        .replace("{{TOPBAR}}", "")
        .replace("{{CONTENT}}", include_str!("ui/dashboard/website.html"));
    Html(page)
}

pub async fn create_website_site(
    Json(request): Json<WebsiteCreateRequest>,
) -> Json<OperationStatus> {
    match create_website(&request) {
        Ok(domain) => {
            if request.apply_ssl {
                return match sync_website_routing_now(&domain) {
                    Ok(()) => Json(OperationStatus {
                        status: true,
                        message: format!("Website {domain} created. HTTPS is now active."),
                    }),
                    Err(error) => Json(OperationStatus {
                        status: false,
                        message: format!(
                            "Website {domain} was created, but HTTPS activation failed: {error}"
                        ),
                    }),
                };
            }
            trigger_website_routing_sync(domain.clone());
            Json(OperationStatus {
                status: true,
                message: format!("Website {domain} created. Applying routing in background."),
            })
        }
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

fn trigger_website_routing_sync(domain: String) {
    tokio::task::spawn_blocking(move || {
        if let Err(error) = sync_website_routing_now(&domain) {
            eprintln!("website routing sync warning for {domain}: {error}");
        }
    });
}

fn sync_website_routing_now(domain: &str) -> Result<(), String> {
    let _guard = website_routing_sync_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut registry = dashboard::load_runtime_registry()
        .map_err(|error| format!("Failed to load runtime registry: {error}"))?;
    dashboard::sync_apache_site_bindings(&mut registry)
        .map_err(|error| format!("Failed to synchronize Apache routing: {error}"))?;
    dashboard::save_runtime_registry(&registry)
        .map_err(|error| format!("Failed to save runtime registry for {domain}: {error}"))?;
    Ok(())
}

fn website_routing_sync_lock() -> &'static Mutex<()> {
    WEBSITE_ROUTING_SYNC_LOCK.get_or_init(|| Mutex::new(()))
}

fn website_source_cache() -> &'static Mutex<Option<CachedWebsiteSources>> {
    WEBSITE_SOURCE_CACHE.get_or_init(|| Mutex::new(None))
}

fn default_create_index_php() -> bool {
    true
}

fn default_site_enabled() -> bool {
    true
}

pub async fn save_website_php_binding(
    Json(request): Json<WebsitePhpBindingRequest>,
) -> Json<OperationStatus> {
    match persist_site_php_binding(&request.site_id, &request.php_runtime_id) {
        Ok(_) => Json(OperationStatus {
            status: true,
            message: "Website PHP binding saved".to_string(),
        }),
        Err(error) => Json(OperationStatus {
            status: false,
            message: error,
        }),
    }
}

pub async fn delete_website_site(
    Json(request): Json<WebsiteDeleteRequest>,
) -> Json<OperationStatus> {
    match delete_website(&request.site_id, request.delete_document_root) {
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

pub async fn start_website_site(
    Json(request): Json<WebsiteLifecycleRequest>,
) -> Json<OperationStatus> {
    match set_website_enabled(&request.site_id, true) {
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

pub async fn pause_website_site(
    Json(request): Json<WebsiteLifecycleRequest>,
) -> Json<OperationStatus> {
    match set_website_enabled(&request.site_id, false) {
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

pub(crate) fn collect_websites(registry: &RuntimeRegistry) -> Vec<WebsiteEntry> {
    let sources = collect_website_sources();
    let mut websites = Vec::new();
    let bindings = load_website_bindings().unwrap_or_default();
    let php_registry = registry
        .entries
        .iter()
        .filter(|entry| entry.runtime_kind == "php" && dashboard::is_runtime_entry_ready(entry))
        .map(|entry| (dashboard::runtime_binding_id(entry), entry.clone()))
        .collect::<HashMap<_, _>>();

    for source in sources {
        let index = websites.len();
        let binding_record = bindings
            .entries
            .iter()
            .find(|binding| binding.site_id == source.id);
        let site_enabled = binding_record
            .map(|binding| binding.enabled)
            .unwrap_or(true);
        let status = if site_enabled { "running" } else { "stopped" };
        let backup_total = source.scan.backup_files;

        let php_binding = binding_record
            .map(|binding| binding.php_runtime_id.clone())
            .filter(|binding_id| !binding_id.trim().is_empty())
            .and_then(|binding_id| {
                dashboard::resolve_php_runtime_binding_id(&binding_id, &php_registry)
            });
        let category = if php_binding.is_some() {
            "PHP Project".to_string()
        } else {
            source.category.clone()
        };
        let runtime = if category == "PHP Project" {
            php_binding
                .as_ref()
                .and_then(|binding_id| php_registry.get(binding_id))
                .map(|runtime| runtime.version.clone())
                .unwrap_or_else(|| runtime_label(index, &source.scan))
        } else {
            runtime_label(index, &source.scan)
        };
        let domain = binding_record
            .map(|binding| binding.domain.trim().to_string())
            .filter(|domain| !domain.is_empty())
            .unwrap_or_else(|| source.domain.clone());
        let alias = binding_record
            .map(|binding| binding.description.trim().to_string())
            .filter(|description| !description.is_empty())
            .unwrap_or_else(|| source.alias.clone());
        let ssl_state = inspect_site_ssl_state(&domain);
        let traffic_state = inspect_site_requests(&source.id);

        websites.push(WebsiteEntry {
            id: source.id.clone(),
            name: domain,
            alias,
            category,
            status: status.to_string(),
            backup_total,
            backup_label: if backup_total == 0 {
                "Not backed up".to_string()
            } else {
                "Backup available".to_string()
            },
            runtime,
            expiration: ssl_state.expiration,
            ssl_status: ssl_state.status,
            ssl_enabled: ssl_state.https_available,
            requests: traffic_state.requests,
            waf: "Active".to_string(),
            php_binding,
        });
    }

    if websites.is_empty() {
        ensure_www_index_php();
    }

    websites
}

fn ensure_www_index_php() {
    let www_root = resolve_website_root();
    let index_path = www_root.join("index.php");
    if !index_path.exists() {
        let _ = fs::create_dir_all(&www_root);
        let _ = fs::write(index_path, "<?php echo 'MinPanel: domain empty'; ?>");
    }
    let index_html_path = www_root.join("index.html");
    if index_html_path.exists() {
        let _ = fs::remove_file(index_html_path);
    }
}

pub(crate) fn collect_website_sources() -> Vec<WebsiteSource> {
    let bindings = load_website_bindings().unwrap_or_default();
    let roots = collect_candidate_website_roots();
    let cache_key = website_sources_cache_key(&bindings, &roots);
    if let Some(cached) = website_source_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|cached| cached.key == cache_key)
        .cloned()
    {
        return cached.sources;
    }

    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if !file_type.is_dir() {
                continue;
            }

            let name = entry.file_name().to_string_lossy().to_string();
            if is_reserved_site_directory(&root, &name) {
                continue;
            }

            let path = entry.path();
            let binding_record = bindings.entries.iter().find(|binding| {
                !binding.site_path.trim().is_empty() && PathBuf::from(&binding.site_path) == path
            });
            let site_id = binding_record
                .map(|binding| binding.site_id.clone())
                .unwrap_or_else(|| dashboard::slugify(&name, '-'));
            if site_id.is_empty() || !seen.insert(site_id.clone()) {
                continue;
            }

            let scan = scan_website_directory(&path);
            let category = if binding_record
                .map(|binding| !binding.php_runtime_id.trim().is_empty())
                .unwrap_or(false)
            {
                "PHP Project".to_string()
            } else {
                detect_website_category(&name, &scan)
            };
            let domain = binding_record
                .map(|binding| binding.domain.trim().to_string())
                .filter(|domain| !domain.is_empty())
                .and_then(|domain| normalize_local_domain_candidate(&domain))
                .unwrap_or_else(|| {
                    normalize_local_domain_candidate(&name)
                        .unwrap_or_else(|| format!("{site_id}{DEFAULT_LOCAL_DOMAIN_SUFFIX}"))
                });
            let alias = binding_record
                .map(|binding| binding.description.trim().to_string())
                .filter(|description| !description.is_empty())
                .unwrap_or_else(|| dashboard::slugify(&name, '_'));
            sources.push(WebsiteSource {
                id: site_id.clone(),
                domain,
                alias,
                category,
                path,
                scan,
            });
        }
    }

    for binding in bindings.entries {
        if binding.site_id.is_empty() || !seen.insert(binding.site_id.clone()) {
            continue;
        }
        if binding.site_path.trim().is_empty() {
            continue;
        }

        let path = PathBuf::from(&binding.site_path);
        if !path.is_dir() {
            continue;
        }

        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(&binding.site_id)
            .to_string();
        let scan = scan_website_directory(&path);
        let category = if !binding.php_runtime_id.trim().is_empty() {
            "PHP Project".to_string()
        } else {
            detect_website_category(&name, &scan)
        };
        let domain = if binding.domain.trim().is_empty() {
            normalize_local_domain_candidate(&name)
                .unwrap_or_else(|| format!("{}{DEFAULT_LOCAL_DOMAIN_SUFFIX}", binding.site_id))
        } else {
            normalize_local_domain_candidate(&binding.domain).unwrap_or(binding.domain.clone())
        };
        sources.push(WebsiteSource {
            id: binding.site_id.clone(),
            domain,
            alias: dashboard::slugify(&name, '_'),
            category,
            path,
            scan,
        });
    }

    sources.sort_by(|left, right| left.domain.cmp(&right.domain));
    *website_source_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(CachedWebsiteSources {
        key: cache_key,
        sources: sources.clone(),
    });
    sources
}

fn website_sources_cache_key(bindings: &WebsiteBindingStore, roots: &[PathBuf]) -> u64 {
    let mut parts = Vec::new();
    parts.push(serde_json::to_string(bindings).unwrap_or_default());

    for root in roots {
        parts.push(format!("root:{}", root.display()));
        let mut entries = fs::read_dir(root)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let file_type = entry.file_type().ok()?;
                if !file_type.is_dir() {
                    return None;
                }

                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs())
                    .unwrap_or_default();
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                Some(format!("{name}:{modified}"))
            })
            .collect::<Vec<_>>();
        entries.sort();
        parts.extend(entries);
    }

    fast_hash(&parts)
}

fn fast_hash<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn collect_candidate_website_roots() -> Vec<PathBuf> {
    vec![resolve_website_root()]
}

pub(crate) fn resolve_website_root() -> PathBuf {
    let path = if let Some(override_path) =
        dashboard::resolve_env_path_override("MINPANEL_WEBSITE_ROOT")
    {
        override_path
    } else {
        dashboard::resolve_data_base_dir()
            .map(|base_dir| base_dir.join("www"))
            .or_else(|| {
                env::current_dir()
                    .ok()
                    .map(|current_dir| current_dir.join("www"))
            })
            .unwrap_or_else(|| PathBuf::from("www"))
    };

    let _ = fs::create_dir_all(&path);
    path
}

fn is_reserved_site_directory(root: &Path, name: &str) -> bool {
    if name.starts_with('.') || name.eq_ignore_ascii_case("target") {
        return true;
    }

    let is_workspace_root = env::current_dir()
        .ok()
        .map(|current_dir| current_dir == root)
        .unwrap_or(false);
    if !is_workspace_root {
        return false;
    }

    matches!(
        name.to_ascii_lowercase().as_str(),
        "aapanel" | "src" | "data"
    )
}

fn scan_website_directory(path: &Path) -> WebsiteScan {
    let mut scan = WebsiteScan {
        item_count: 0,
        backup_files: 0,
        has_php: false,
        has_node: false,
        has_python: false,
        has_proxy: false,
    };

    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return scan,
    };

    for entry in entries.flatten() {
        scan.item_count += 1;
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();

        if name.contains("backup")
            || name.ends_with(".zip")
            || name.ends_with(".tar")
            || name.ends_with(".gz")
            || name.ends_with(".sql")
        {
            scan.backup_files += 1;
        }

        if name.ends_with(".php") || name == "composer.json" {
            scan.has_php = true;
        }
        if name.ends_with(".py") || name == "requirements.txt" || name == "pyproject.toml" {
            scan.has_python = true;
        }
        if name.ends_with(".js")
            || name.ends_with(".ts")
            || name == "package.json"
            || name == "node_modules"
        {
            scan.has_node = true;
        }
        if name.contains("proxy") || name == "nginx.conf" || name == "caddyfile" {
            scan.has_proxy = true;
        }
    }

    scan
}

fn detect_website_category(name: &str, scan: &WebsiteScan) -> String {
    let lower = name.to_ascii_lowercase();

    if scan.has_node || lower.contains("node") {
        return "Node Project".to_string();
    }
    if scan.has_python || lower.contains("python") || lower.starts_with("py") {
        return "Python Project".to_string();
    }
    if scan.has_proxy || lower.contains("proxy") {
        return "Proxy Project".to_string();
    }
    "PHP Project".to_string()
}

fn runtime_label(index: usize, scan: &WebsiteScan) -> String {
    if scan.has_node {
        return "Node 22".to_string();
    }
    if scan.has_python {
        return "3.12".to_string();
    }
    if scan.has_proxy {
        return "Proxy".to_string();
    }
    if scan.has_php {
        return "PHP".to_string();
    }

    let _ = index;
    "Static".to_string()
}

pub(crate) fn load_website_bindings() -> Result<WebsiteBindingStore, String> {
    let path = website_binding_path()?;
    if !path.exists() {
        return Ok(WebsiteBindingStore::default());
    }
    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read website bindings: {error}"))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("Failed to parse website bindings: {error}"))
}

pub(crate) fn save_website_bindings(store: &WebsiteBindingStore) -> Result<(), String> {
    let path = website_binding_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create website binding directory: {error}"))?;
    }
    let body = serde_json::to_string_pretty(store)
        .map_err(|error| format!("Failed to encode website bindings: {error}"))?;
    fs::write(path, body).map_err(|error| format!("Failed to write website bindings: {error}"))?;
    invalidate_website_source_cache();
    Ok(())
}

fn website_binding_path() -> Result<PathBuf, String> {
    let base_dir = dashboard::resolve_data_base_dir()
        .ok_or_else(|| "Unable to resolve application directory".to_string())?;
    Ok(base_dir.join("data").join("registry").join("site_php.json"))
}

fn build_site_binding(source: &WebsiteSource) -> WebsitePhpBinding {
    WebsitePhpBinding {
        site_id: source.id.clone(),
        php_runtime_id: String::new(),
        domain: source.domain.clone(),
        description: source.alias.clone(),
        site_path: source.path.display().to_string(),
        ssl: ssl_paths_for_domain(&source.domain).is_some(),
        enabled: true,
    }
}

fn sync_website_runtime_registry() -> Result<(), String> {
    let mut registry = dashboard::load_runtime_registry()?;
    dashboard::sync_apache_site_bindings(&mut registry)?;
    dashboard::save_runtime_registry(&registry)
}

fn persist_site_php_binding(site_id: &str, php_runtime_id: &str) -> Result<(), String> {
    let mut store = load_website_bindings().unwrap_or_default();
    if php_runtime_id.trim().is_empty() {
        if let Some(binding) = store
            .entries
            .iter_mut()
            .find(|binding| binding.site_id == site_id)
        {
            binding.php_runtime_id.clear();
        }
        save_website_bindings(&store)?;
        sync_website_runtime_registry()?;
        return Ok(());
    }

    let mut registry = dashboard::load_runtime_registry()?;
    let runtime = registry
        .entries
        .iter()
        .find(|entry| {
            entry.runtime_kind == "php" && dashboard::runtime_binding_id(entry) == php_runtime_id
        })
        .ok_or_else(|| "Selected PHP runtime is not installed".to_string())?;
    if runtime.state != "running" && runtime.state != "stopped" {
        return Err("Selected PHP runtime is not ready".to_string());
    }
    if runtime.php_port.is_none() {
        return Err("Selected PHP runtime has no assigned FastCGI port".to_string());
    }

    if let Some(binding) = store
        .entries
        .iter_mut()
        .find(|binding| binding.site_id == site_id)
    {
        binding.php_runtime_id = php_runtime_id.to_string();
    } else {
        store.entries.push(WebsitePhpBinding {
            site_id: site_id.to_string(),
            php_runtime_id: php_runtime_id.to_string(),
            domain: String::new(),
            description: String::new(),
            site_path: String::new(),
            ssl: false,
            enabled: true,
        });
    }
    save_website_bindings(&store)?;
    dashboard::sync_apache_site_bindings(&mut registry)?;
    dashboard::save_runtime_registry(&registry)?;
    Ok(())
}

fn set_website_enabled(site_id: &str, enabled: bool) -> Result<String, String> {
    let site_id = site_id.trim();
    if site_id.is_empty() {
        return Err("Website ID is required".to_string());
    }

    let sources = collect_website_sources();
    let source = sources
        .iter()
        .find(|entry| entry.id == site_id)
        .ok_or_else(|| "Website not found".to_string())?;

    let mut store = load_website_bindings().unwrap_or_default();
    if let Some(binding) = store
        .entries
        .iter_mut()
        .find(|binding| binding.site_id == site_id)
    {
        if binding.domain.trim().is_empty() {
            binding.domain = source.domain.clone();
        }
        if binding.description.trim().is_empty() {
            binding.description = source.alias.clone();
        }
        if binding.site_path.trim().is_empty() {
            binding.site_path = source.path.display().to_string();
        }
        binding.ssl = binding.ssl || ssl_paths_for_domain(&source.domain).is_some();
        binding.enabled = enabled;
    } else {
        let mut binding = build_site_binding(source);
        binding.enabled = enabled;
        store.entries.push(binding);
    }

    save_website_bindings(&store)?;
    sync_website_runtime_registry()?;

    Ok(if enabled {
        format!("Website {} started", source.domain)
    } else {
        format!("Website {} paused", source.domain)
    })
}

fn delete_website(site_id: &str, delete_document_root: bool) -> Result<String, String> {
    let site_id = site_id.trim();
    if site_id.is_empty() {
        return Err("Website ID is required".to_string());
    }

    let mut store = load_website_bindings().unwrap_or_default();
    let binding_index = store
        .entries
        .iter()
        .position(|binding| binding.site_id == site_id);
    let sources = collect_website_sources();
    let source = sources.iter().find(|entry| entry.id == site_id);

    if binding_index.is_none() && source.is_none() {
        return Err("Website not found".to_string());
    }

    let binding = binding_index.map(|index| store.entries.remove(index));
    let site_path = binding
        .as_ref()
        .map(|entry| PathBuf::from(entry.site_path.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| source.map(|entry| entry.path.clone()));
    let domain = binding
        .as_ref()
        .map(|entry| entry.domain.trim().to_string())
        .filter(|domain| !domain.is_empty())
        .or_else(|| source.map(|entry| entry.domain.clone()))
        .unwrap_or_else(|| site_id.to_string());

    let mut preserved_files =
        site_path.as_ref().filter(|path| path.exists()).is_some() && !delete_document_root;
    if let Some(path) = site_path.as_ref().filter(|path| path.exists()) {
        if !delete_document_root {
            preserved_files = true;
        } else if should_delete_managed_website_path(path) {
            fs::remove_dir_all(path).map_err(|error| {
                format!(
                    "Failed to delete website directory {}: {error}",
                    path.display()
                )
            })?;
        } else {
            preserved_files = true;
        }
    }

    save_website_bindings(&store)?;

    let mut registry = dashboard::load_runtime_registry()?;
    dashboard::sync_apache_site_bindings(&mut registry)?;
    dashboard::save_runtime_registry(&registry)?;

    if preserved_files {
        Ok(format!(
            "Website {domain} deleted from MinPanel. Files outside the managed www root were kept."
        ))
    } else {
        Ok(format!("Website {domain} deleted"))
    }
}

fn should_delete_managed_website_path(site_path: &Path) -> bool {
    let root = resolve_website_root();
    let root = match fs::canonicalize(&root) {
        Ok(path) => path,
        Err(_) => return false,
    };
    let site = match fs::canonicalize(site_path) {
        Ok(path) => path,
        Err(_) => return false,
    };

    site != root && site.starts_with(&root)
}

fn invalidate_website_source_cache() {
    *website_source_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

fn create_website(request: &WebsiteCreateRequest) -> Result<String, String> {
    let domain = normalize_requested_website_domain(&request.domain)?;
    let site_id = dashboard::slugify(&domain, '-');
    if site_id.is_empty() {
        return Err("Domain name is invalid".to_string());
    }
    ensure_local_domain_mapping(&domain)?;

    let website_root = resolve_website_root();
    fs::create_dir_all(&website_root)
        .map_err(|error| format!("Failed to create website root: {error}"))?;
    let site_path = if request.website_path.trim().is_empty() {
        website_root.join(&domain)
    } else {
        let requested = PathBuf::from(request.website_path.trim());
        if requested.is_absolute() {
            requested
        } else {
            website_root.join(requested)
        }
    };
    if site_path.as_os_str().is_empty() || site_path.components().count() <= 1 {
        return Err("Please fill in the website path".to_string());
    }
    if site_path.exists() && !site_path.is_dir() {
        return Err("The specified website path is not a directory".to_string());
    }

    let site_preexisted = site_path.exists();
    fs::create_dir_all(&site_path)
        .map_err(|error| format!("Failed to create site document root, {error}"))?;
    if request.create_html {
        ensure_site_index_php(&site_path, site_preexisted)?;
    }

    let registry = dashboard::load_runtime_registry()?;
    let requested_php_runtime_id = request.php_runtime_id.trim();
    let php_runtime_id = if requested_php_runtime_id.is_empty() {
        registry
            .entries
            .iter()
            .filter(|entry| entry.runtime_kind == "php" && dashboard::is_runtime_entry_ready(entry))
            .max_by(|left, right| left.version.cmp(&right.version))
            .map(dashboard::runtime_binding_id)
            .unwrap_or_default()
    } else {
        requested_php_runtime_id.to_string()
    };
    if !php_runtime_id.is_empty() {
        let runtime = registry
            .entries
            .iter()
            .find(|entry| {
                entry.runtime_kind == "php"
                    && dashboard::runtime_binding_id(entry) == php_runtime_id
            })
            .ok_or_else(|| "Selected PHP runtime is not installed".to_string())?;
        if runtime.php_port.is_none() {
            return Err("Selected PHP runtime has no assigned FastCGI port".to_string());
        }
    }

    let mut store = load_website_bindings().unwrap_or_default();
    if let Some(binding) = store
        .entries
        .iter_mut()
        .find(|binding| binding.site_id == site_id)
    {
        binding.php_runtime_id = php_runtime_id;
        binding.domain = domain.clone();
        binding.description = request.description.trim().to_string();
        binding.site_path = site_path.display().to_string();
        binding.ssl = request.apply_ssl;
        binding.enabled = true;
    } else {
        store.entries.push(WebsitePhpBinding {
            site_id: site_id.clone(),
            php_runtime_id,
            domain: domain.clone(),
            description: request.description.trim().to_string(),
            site_path: site_path.display().to_string(),
            ssl: request.apply_ssl,
            enabled: true,
        });
    }
    save_website_bindings(&store)?;

    if request.apply_ssl {
        apply_ssl_for_domain(&domain)
            .map_err(|ssl_err| format!("SSL setup failed for {domain}: {ssl_err}"))?;
    }

    Ok(domain)
}

fn ensure_site_index_php(site_path: &Path, preserve_existing_html: bool) -> Result<(), String> {
    let index_file = site_path.join("index.php");
    let favicon_file = site_path.join("favicon.png");

    if !favicon_file.exists() {
        fs::write(&favicon_file, include_bytes!("ui/dashboard/favicon.png"))
            .map_err(|error| format!("Failed to create default favicon: {error}"))?;
    }

    if !index_file.exists() {
        fs::write(
            &index_file,
            r#"<?php
// ==========================
// BASIC ENV DETECTION
// ==========================
$isLocal = in_array($_SERVER['REMOTE_ADDR'], ['127.0.0.1', '::1'], true);

// ==========================
// QUERY HANDLING (SAFE)
// ==========================
if (isset($_GET['q'])) {
    $query = $_GET['q'];

    // Allow-list approach
    if ($query === 'info') {

        // phpinfo allowed ONLY on localhost
        if ($isLocal) {
            phpinfo();
            exit;
        }

        http_response_code(403);
        exit('Forbidden');
    }

    // Unknown query
    http_response_code(404);
    exit('Invalid query parameter.');
}
?>

<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>MinPanel</title>
    <link rel="icon" href="/favicon.png?v=1" type="image/png">
    <link rel="apple-touch-icon" href="/favicon.png?v=1">

    <link href="https://fonts.googleapis.com/css?family=Karla:400" rel="stylesheet">

    <style>
        html, body {
            height: 100%;
            margin: 0;
            padding: 0;
            font-family: 'Karla', sans-serif;
            background-color: #f9f9f9;
            color: #333;
        }

        .container {
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100%;
            text-align: center;
        }

        .content {
            max-width: 800px;
            padding: 100px;
            background: #fff;
            border-radius: 12px;
            border: 1px solid #24b24a;
            border-top-width: 5px;
            box-shadow: 0 10px 25px rgba(0, 0, 0, 0.05);
        }

        .title {
            font-size: 60px;
            margin: 0;
        }

        .info {
            margin-top: 20px;
            font-size: 18px;
            line-height: 1.6;
        }

        .info a {
            color: #24b24a;
            text-decoration: none;
        }

        .info a:hover {
            color: #1a8a38;
            text-decoration: underline;
        }

        .opt {
            margin-top: 30px;
        }

        .opt a {
            font-size: 18px;
            color: #24b24a;
            text-decoration: none;
        }

        .opt a:hover {
            color: #1a8a38;
            text-decoration: underline;
        }
    </style>
</head>
<body>

<div class="container">
    <div class="content">
        <h1 class="title">MinPanel</h1>

        <div class="info">
            <?php if ($isLocal): ?>
                <p><?= htmlspecialchars($_SERVER['SERVER_SOFTWARE'], ENT_QUOTES, 'UTF-8'); ?></p>
                <p>
                    PHP version: <?= htmlspecialchars(PHP_VERSION, ENT_QUOTES, 'UTF-8'); ?>
                    <a title="phpinfo()" href="/?q=info">info</a>
                </p>
                <p>
                    Document Root:
                    <?= htmlspecialchars($_SERVER['DOCUMENT_ROOT'], ENT_QUOTES, 'UTF-8'); ?>
                </p>
            <?php else: ?>
                <p>Server is running</p>
                <p>PHP is enabled</p>
            <?php endif; ?>
        </div>

        <div class="opt">
            <p>
                <a href="https://hocdev.com" target="_blank" rel="noopener">
                    Getting Started
                </a>
            </p>
        </div>
    </div>
</div>

</body>
</html>"#,
        )
        .map_err(|error| format!("Failed to create default php file: {error}"))?;
    }

    if !preserve_existing_html {
        let index_html = site_path.join("index.html");
        if index_html.exists() {
            fs::remove_file(&index_html)
                .map_err(|error| format!("Failed to remove index.html: {error}"))?;
        }
    }

    Ok(())
}

fn normalize_requested_website_domain(raw: &str) -> Result<String, String> {
    let domain = raw
        .lines()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .ok_or_else(|| "Please enter the domain name".to_string())?;

    normalize_local_domain_candidate(domain).ok_or_else(|| "Domain name is invalid".to_string())
}

fn normalize_local_domain_candidate(raw: &str) -> Option<String> {
    let normalized = finalize_local_domain_input(raw);
    if normalized.is_empty() || !is_valid_local_domain(&normalized) {
        return None;
    }
    Some(normalized)
}

fn finalize_local_domain_input(value: &str) -> String {
    let draft = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    if should_append_local_domain_suffix(&draft) {
        return format!("{draft}{DEFAULT_LOCAL_DOMAIN_SUFFIX}");
    }
    draft
}

fn should_append_local_domain_suffix(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('.')
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn is_valid_local_domain(domain: &str) -> bool {
    if !(3..=253).contains(&domain.len()) {
        return false;
    }
    if domain.starts_with('.') || domain.ends_with('.') {
        return false;
    }

    let parts = domain.split('.').collect::<Vec<_>>();
    if parts.len() < 2 {
        return false;
    }

    parts.into_iter().all(|part| {
        !part.is_empty()
            && part.len() <= 63
            && part.chars().enumerate().all(|(index, character)| {
                character.is_ascii_alphanumeric()
                    || (character == '-' && index > 0 && index + 1 < part.len())
            })
    })
}

#[cfg(windows)]
fn ensure_local_domain_mapping(domain: &str) -> Result<(), String> {
    let content = fs::read_to_string(WINDOWS_HOSTS_FILE)
        .map_err(|error| format!("Failed to read Windows hosts file: {error}"))?;
    let Some(updated) = hosts_content_with_domain(&content, domain) else {
        return Ok(());
    };

    match fs::write(WINDOWS_HOSTS_FILE, &updated) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            request_elevated_windows_hosts_update(&updated, domain)
        }
        Err(error) => Err(format!("Failed to update Windows hosts file: {error}")),
    }
}

#[cfg(not(windows))]
fn ensure_local_domain_mapping(_domain: &str) -> Result<(), String> {
    Ok(())
}

fn hosts_content_with_domain(content: &str, domain: &str) -> Option<String> {
    let normalized_domain = domain.trim().to_ascii_lowercase();
    if normalized_domain.is_empty() {
        return None;
    }

    let mut lines = content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let fields = trimmed.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 2 {
            continue;
        }

        if fields[1..]
            .iter()
            .any(|field| field.eq_ignore_ascii_case(&normalized_domain))
        {
            return None;
        }
    }

    if lines
        .last()
        .map(|line| !line.trim().is_empty())
        .unwrap_or(false)
    {
        lines.push(String::new());
    }
    lines.push(format!("127.0.0.1 {normalized_domain}"));
    Some(lines.join("\r\n"))
}

#[cfg(windows)]
fn request_elevated_windows_hosts_update(updated: &str, domain: &str) -> Result<(), String> {
    let source_path = write_pending_hosts_file(updated, domain)?;
    let status_path = write_hosts_update_status_path(domain)?;
    let script_path = resolve_windows_hosts_update_script()?;
    let args = format!(
        "/d /s /c \"\"{}\" \"{}\" \"{}\" \"{}\"\"",
        script_path.display(),
        source_path.display(),
        WINDOWS_HOSTS_FILE,
        status_path.display()
    );

    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            to_wide("runas").as_ptr(),
            to_wide(WINDOWS_CMD_EXECUTABLE).as_ptr(),
            to_wide(&args).as_ptr(),
            null(),
            SW_HIDE,
        )
    } as isize;

    if result <= 32 {
        let _ = fs::remove_file(&source_path);
        let _ = fs::remove_file(&status_path);
        return Err(format!(
            "MinPanel needs Windows permission to update the hosts file for {domain}. Approve the UAC prompt and try again if needed."
        ));
    }

    for _ in 0..75 {
        thread::sleep(Duration::from_millis(200));
        if let Some(status) = read_hosts_update_status(&status_path) {
            let _ = fs::remove_file(&source_path);
            let _ = fs::remove_file(&status_path);
            if status.eq_ignore_ascii_case("ok") {
                return Ok(());
            }
            return Err(format!(
                "MinPanel could not update the Windows hosts file for {domain}: {status}"
            ));
        }
        let content = match fs::read_to_string(WINDOWS_HOSTS_FILE) {
            Ok(content) => content,
            Err(_) => continue,
        };
        if hosts_content_with_domain(&content, domain).is_none() {
            let _ = fs::remove_file(&source_path);
            let _ = fs::remove_file(&status_path);
            return Ok(());
        }
    }

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&status_path);
    Err(format!(
            "MinPanel requested elevated access to update the Windows hosts file for {domain}, but the helper did not report completion."
    ))
}

#[cfg(windows)]
fn write_pending_hosts_file(updated: &str, domain: &str) -> Result<PathBuf, String> {
    let staging_dir = ensure_hosts_update_staging_dir()?;
    let path = staging_dir.join(format!(
        "hosts-{}-{}.tmp",
        dashboard::slugify(domain, '-'),
        std::process::id()
    ));
    fs::write(&path, updated).map_err(|error| format!("Failed to stage hosts update: {error}"))?;
    Ok(path)
}

#[cfg(windows)]
fn write_hosts_update_status_path(domain: &str) -> Result<PathBuf, String> {
    let staging_dir = ensure_hosts_update_staging_dir()?;
    let path = staging_dir.join(format!(
        "hosts-{}-{}.status",
        dashboard::slugify(domain, '-'),
        std::process::id()
    ));
    let _ = fs::remove_file(&path);
    Ok(path)
}

#[cfg(windows)]
fn ensure_hosts_update_staging_dir() -> Result<PathBuf, String> {
    let base_dir = dashboard::resolve_data_base_dir()
        .ok_or_else(|| "Unable to resolve application directory".to_string())?;
    let staging_dir = base_dir.join("data").join("staging");
    fs::create_dir_all(&staging_dir)
        .map_err(|error| format!("Failed to prepare hosts update staging directory: {error}"))?;
    Ok(staging_dir)
}

#[cfg(windows)]
fn read_hosts_update_status(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(windows)]
fn resolve_windows_hosts_update_script() -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Ok(executable) = env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    if let Ok(current_dir) = env::current_dir() {
        candidates.push(current_dir);
    }

    for candidate in candidates {
        let script = candidate.join(WINDOWS_HOSTS_UPDATE_SCRIPT);
        if script.exists() {
            return Ok(script);
        }
        if let Some(root) = find_workspace_root(&candidate) {
            let script = root.join(WINDOWS_HOSTS_UPDATE_SCRIPT);
            if script.exists() {
                return Ok(script);
            }
        }
    }

    Err(format!(
        "Windows hosts update helper was not found at {}",
        WINDOWS_HOSTS_UPDATE_SCRIPT
    ))
}

#[cfg(windows)]
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

#[cfg(windows)]
fn to_wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

// ─── SSL Support ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct SslRequest {
    pub site_id: String,
}

/// HTTP handler: POST /website/ssl
/// Applies SSL for an existing site identified by site_id.
pub async fn apply_website_ssl_handler(
    Json(request): Json<SslRequest>,
) -> Json<crate::dashboard::OperationStatus> {
    let bindings = load_website_bindings().unwrap_or_default();
    let Some(binding) = bindings
        .entries
        .iter()
        .find(|b| b.site_id == request.site_id)
    else {
        return Json(crate::dashboard::OperationStatus {
            status: false,
            message: "Site not found".to_string(),
        });
    };
    let domain = binding.domain.clone();
    match apply_ssl_for_domain(&domain) {
        Ok(()) => {
            // Persist ssl=true in binding store
            let mut store = bindings;
            if let Some(b) = store
                .entries
                .iter_mut()
                .find(|b| b.site_id == request.site_id)
            {
                b.ssl = true;
            }
            match save_website_bindings(&store) {
                Ok(()) => match sync_website_routing_now(&domain) {
                    Ok(()) => Json(crate::dashboard::OperationStatus {
                        status: true,
                        message: format!("SSL applied for {domain}. HTTPS is now active."),
                    }),
                    Err(error) => Json(crate::dashboard::OperationStatus {
                        status: false,
                        message: format!(
                            "SSL was generated for {domain}, but Apache could not enable HTTPS: {error}"
                        ),
                    }),
                },
                Err(error) => Json(crate::dashboard::OperationStatus {
                    status: false,
                    message: format!("SSL was generated for {domain}, but MinPanel could not save the site binding: {error}"),
                }),
            }
        }
        Err(e) => Json(crate::dashboard::OperationStatus {
            status: false,
            message: format!("SSL setup failed: {e}"),
        }),
    }
}

/// Returns the directory where MinPanel stores SSL certificates.
pub(crate) fn ssl_cert_dir() -> Option<PathBuf> {
    dashboard::resolve_data_base_dir().map(|base| base.join("data").join("ssl"))
}

/// Returns `(cert_path, key_path)` for a given domain, or None if certs don't exist.
pub(crate) fn ssl_paths_for_domain(domain: &str) -> Option<(PathBuf, PathBuf)> {
    let dir = ssl_cert_dir()?;
    let cert = dir.join(format!("{domain}.crt"));
    let key = dir.join(format!("{domain}.key"));
    if cert.exists() && key.exists() {
        Some((cert, key))
    } else {
        None
    }
}

fn inspect_site_ssl_state(domain: &str) -> SiteSslState {
    #[cfg(windows)]
    {
        return inspect_site_ssl_state_windows(domain);
    }

    #[cfg(not(windows))]
    {
        let https_available = ssl_paths_for_domain(domain).is_some();
        return SiteSslState {
            expiration: if https_available {
                "--".to_string()
            } else {
                "No SSL".to_string()
            },
            status: if https_available {
                "Valid".to_string()
            } else {
                "None".to_string()
            },
            https_available,
        };
    }
}

fn inspect_site_requests(site_id: &str) -> SiteTrafficState {
    let Some(base_dir) = dashboard::resolve_data_base_dir() else {
        return SiteTrafficState { requests: 0 };
    };

    let site_log_base = dashboard::sanitize_path_segment(site_id);
    let logs_root = base_dir
        .join("data")
        .join("logs")
        .join("apache")
        .join("sites");
    let requests = [
        logs_root.join(format!("{site_log_base}-access.log")),
        logs_root.join(format!("{site_log_base}-ssl-access.log")),
    ]
    .into_iter()
    .map(|path| count_log_lines(&path))
    .sum();

    SiteTrafficState { requests }
}

fn count_log_lines(path: &Path) -> u64 {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return 0,
    };

    io::BufReader::new(file)
        .lines()
        .filter(|line| line.is_ok())
        .count() as u64
}

#[cfg(windows)]
fn inspect_site_ssl_state_windows(domain: &str) -> SiteSslState {
    let Some((cert_path, _key_path)) = ssl_paths_for_domain(domain) else {
        return SiteSslState {
            expiration: "No SSL".to_string(),
            status: "None".to_string(),
            https_available: false,
        };
    };

    let openssl_dir = match bundled_openssl_dir() {
        Ok(path) => path,
        Err(_) => {
            return SiteSslState {
                expiration: "--".to_string(),
                status: "Invalid".to_string(),
                https_available: true,
            };
        }
    };
    let openssl_exe = match openssl_executable(&openssl_dir) {
        Ok(path) => path,
        Err(_) => {
            return SiteSslState {
                expiration: "--".to_string(),
                status: "Invalid".to_string(),
                https_available: true,
            };
        }
    };

    let enddate_output = match std::process::Command::new(&openssl_exe)
        .args(["x509", "-in"])
        .arg(&cert_path)
        .args(["-noout", "-enddate"])
        .creation_flags(WINDOWS_CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) => output,
        Err(_) => {
            return SiteSslState {
                expiration: "--".to_string(),
                status: "Invalid".to_string(),
                https_available: true,
            };
        }
    };

    if !enddate_output.status.success() {
        return SiteSslState {
            expiration: "--".to_string(),
            status: "Invalid".to_string(),
            https_available: true,
        };
    }

    let raw_enddate = String::from_utf8_lossy(&enddate_output.stdout);
    let expiration =
        format_certificate_expiration_label(&raw_enddate).unwrap_or_else(|| "--".to_string());

    let is_valid_now = std::process::Command::new(&openssl_exe)
        .args(["x509", "-in"])
        .arg(&cert_path)
        .args(["-noout", "-checkend", "0"])
        .creation_flags(WINDOWS_CREATE_NO_WINDOW)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);

    SiteSslState {
        expiration,
        status: if is_valid_now {
            "Valid".to_string()
        } else {
            "Expired".to_string()
        },
        https_available: true,
    }
}

fn format_certificate_expiration_label(raw_output: &str) -> Option<String> {
    let raw_date = raw_output
        .lines()
        .find_map(|line| line.trim().strip_prefix("notAfter="))?
        .trim();
    let parts = raw_date.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 4 {
        return None;
    }

    let month = match parts[0] {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let day = parts[1].parse::<u32>().ok()?;
    let year = parts[3].parse::<u32>().ok()?;
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// Returns true if the binding store says this site has SSL enabled.
#[allow(dead_code)]
pub(crate) fn site_has_ssl(site_id: &str) -> bool {
    load_website_bindings()
        .map(|store| store.entries.iter().any(|b| b.site_id == site_id && b.ssl))
        .unwrap_or(false)
}

#[cfg(windows)]
fn bundled_openssl_dir() -> Result<PathBuf, String> {
    let openssl_dir = dashboard::resolve_data_base_dir()
        .ok_or_else(|| "Unable to resolve binary directory".to_string())?
        .join("data")
        .join("bin")
        .join("openssl");

    if openssl_dir.exists() {
        Ok(openssl_dir)
    } else {
        Err(format!(
            "OpenSSL directory not found at {}. Please ensure data/bin/openssl was copied correctly.",
            openssl_dir.display()
        ))
    }
}

#[cfg(windows)]
fn openssl_executable(openssl_dir: &Path) -> Result<PathBuf, String> {
    let openssl_exe = openssl_dir.join("openssl.exe");
    if openssl_exe.exists() {
        Ok(openssl_exe)
    } else {
        Err(format!(
            "OpenSSL executable not found at {}.",
            openssl_exe.display()
        ))
    }
}

#[cfg(windows)]
fn command_output_details(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        return stderr.trim().to_string();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.trim().is_empty() {
        return stdout.trim().to_string();
    }

    format!("exit status {}", output.status)
}

#[cfg(windows)]
fn hide_windows_command_window(command: &mut std::process::Command) {
    command.creation_flags(WINDOWS_CREATE_NO_WINDOW);
}

#[cfg(windows)]
fn run_checked_command(
    command_name: &str,
    command: &mut std::process::Command,
) -> Result<(), String> {
    hide_windows_command_window(command);
    let output = command
        .output()
        .map_err(|e| format!("Failed to run {command_name}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{command_name} failed: {}",
            command_output_details(&output)
        ))
    }
}

#[cfg(windows)]
fn write_temp_ssl_file(prefix: &str, suffix: &str, contents: &str) -> Result<PathBuf, String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = env::temp_dir().join(format!(
        "MinPanel-{prefix}-{}-{timestamp}{suffix}",
        std::process::id()
    ));
    fs::write(&path, contents).map_err(|e| {
        format!(
            "Failed to write temporary OpenSSL config {}: {e}",
            path.display()
        )
    })?;
    Ok(path)
}

#[cfg(windows)]
fn remove_file_if_exists(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(windows)]
fn root_ca_openssl_config() -> &'static str {
    "[ req ]\n\
prompt = no\n\
distinguished_name = req_distinguished_name\n\
x509_extensions = v3_root_ca\n\
\n\
[ req_distinguished_name ]\n\
C = US\n\
O = HocDev\n\
OU = Local Certificate Authority\n\
CN = HocDev Private CA Root\n\
\n\
[ v3_root_ca ]\n\
subjectKeyIdentifier = hash\n\
authorityKeyIdentifier = keyid:always,issuer\n\
basicConstraints = critical, CA:true, pathlen:1\n\
keyUsage = critical, keyCertSign, cRLSign\n"
}

#[cfg(windows)]
fn certificate_request_config() -> &'static str {
    "[ req ]\n\
prompt = no\n\
distinguished_name = req_distinguished_name\n\
\n\
[ req_distinguished_name ]\n\
C = US\n\
O = HocDev\n\
OU = Local Certificate Authority\n\
CN = HocDev Placeholder\n"
}

#[cfg(windows)]
fn intermediate_ca_openssl_config() -> &'static str {
    "[ v3_intermediate_ca ]\n\
subjectKeyIdentifier = hash\n\
authorityKeyIdentifier = keyid:always,issuer\n\
basicConstraints = critical, CA:true, pathlen:0\n\
keyUsage = critical, keyCertSign, cRLSign\n"
}

#[cfg(windows)]
fn site_certificate_openssl_config(domain: &str) -> String {
    format!(
        "[ req ]\n\
default_bits = 2048\n\
distinguished_name = req_distinguished_name\n\
req_extensions = v3_req\n\
prompt = no\n\
\n\
[ req_distinguished_name ]\n\
C = US\n\
O = HocDev\n\
OU = Local Development\n\
CN = {domain}\n\
emailAddress = local-admin@{domain}\n\
\n\
[ v3_req ]\n\
basicConstraints = CA:FALSE\n\
keyUsage = digitalSignature, keyEncipherment\n\
extendedKeyUsage = serverAuth\n\
subjectAltName = @alt_names\n\
\n\
[ alt_names ]\n\
DNS.1 = {domain}\n\
DNS.2 = *.{domain}\n\
DNS.3 = localhost\n"
    )
}

#[cfg(windows)]
fn ensure_local_ssl_ca_exists(openssl_dir: &Path) -> Result<(), String> {
    let root_cert = openssl_dir.join("ca").join("rootCA.pem");
    let root_key = openssl_dir.join("ca").join("rootCA-key.pem");
    let root_serial = openssl_dir.join("ca").join("rootCA.srl");
    let intermediate_cert = openssl_dir.join("ca").join("intermediateCA.pem");
    let intermediate_key = openssl_dir.join("ca").join("intermediateCA-key.pem");
    let intermediate_csr = openssl_dir.join("ca").join("intermediateCA.csr");
    let openssl_exe = openssl_executable(openssl_dir)?;

    if root_cert.exists()
        && root_key.exists()
        && intermediate_cert.exists()
        && intermediate_key.exists()
    {
        return Ok(());
    }

    fs::create_dir_all(openssl_dir.join("ca"))
        .map_err(|e| format!("Failed to create local CA directory: {e}"))?;

    let root_config_path = write_temp_ssl_file("root-ca", ".cnf", root_ca_openssl_config())?;
    let req_config_path = write_temp_ssl_file("ca-req", ".cnf", certificate_request_config())?;
    let intermediate_ext_path =
        write_temp_ssl_file("intermediate-ca", ".cnf", intermediate_ca_openssl_config())?;

    let result = (|| -> Result<(), String> {
        if !root_key.exists() {
            let mut command = std::process::Command::new(&openssl_exe);
            command.args(["ecparam", "-name", "secp384r1", "-genkey", "-noout", "-out"]);
            command.arg(&root_key);
            run_checked_command("OpenSSL root CA key generation", &mut command)?;
        }

        if !root_cert.exists() {
            let mut command = std::process::Command::new(&openssl_exe);
            command.args(["req", "-x509", "-new", "-sha384", "-days", "3650", "-key"]);
            command.arg(&root_key);
            command.args(["-out"]);
            command.arg(&root_cert);
            command.args([
                "-subj",
                "/C=US/O=HocDev/OU=Local Certificate Authority/CN=HocDev Private CA Root",
                "-extensions",
                "v3_root_ca",
                "-config",
            ]);
            command.arg(&root_config_path);
            run_checked_command("OpenSSL root CA certificate generation", &mut command)?;
        }

        if !intermediate_key.exists() {
            let mut command = std::process::Command::new(&openssl_exe);
            command.args(["ecparam", "-name", "secp384r1", "-genkey", "-noout", "-out"]);
            command.arg(&intermediate_key);
            run_checked_command("OpenSSL intermediate CA key generation", &mut command)?;
        }

        if !intermediate_cert.exists() {
            let mut csr_command = std::process::Command::new(&openssl_exe);
            csr_command.args(["req", "-new", "-key"]);
            csr_command.arg(&intermediate_key);
            csr_command.args(["-out"]);
            csr_command.arg(&intermediate_csr);
            csr_command.args([
                "-config",
                req_config_path.to_str().unwrap_or_default(),
                "-subj",
                "/C=US/O=HocDev/OU=Local Certificate Authority/CN=HocDev Private CA Intermediate",
            ]);
            run_checked_command("OpenSSL intermediate CA CSR generation", &mut csr_command)?;

            let mut cert_command = std::process::Command::new(&openssl_exe);
            cert_command.args(["x509", "-req", "-in"]);
            cert_command.arg(&intermediate_csr);
            cert_command.args(["-CA"]);
            cert_command.arg(&root_cert);
            cert_command.args(["-CAkey"]);
            cert_command.arg(&root_key);
            cert_command.args(["-set_serial", "2", "-out"]);
            cert_command.arg(&intermediate_cert);
            cert_command.args([
                "-days",
                "1825",
                "-sha384",
                "-extensions",
                "v3_intermediate_ca",
                "-extfile",
            ]);
            cert_command.arg(&intermediate_ext_path);
            run_checked_command(
                "OpenSSL intermediate CA certificate generation",
                &mut cert_command,
            )?;
        }

        Ok(())
    })();

    remove_file_if_exists(&intermediate_csr);
    remove_file_if_exists(&root_serial);
    remove_file_if_exists(&root_config_path);
    remove_file_if_exists(&req_config_path);
    remove_file_if_exists(&intermediate_ext_path);

    if result.is_err() {
        if !intermediate_cert.exists() {
            remove_file_if_exists(&intermediate_key);
        }
        if !root_cert.exists() {
            remove_file_if_exists(&root_key);
        }
    }
    result?;

    if root_cert.exists()
        && root_key.exists()
        && intermediate_cert.exists()
        && intermediate_key.exists()
    {
        Ok(())
    } else {
        Err("HocDev CA bootstrap finished but the CA files were not created.".to_string())
    }
}

#[cfg(windows)]
fn ensure_windows_ssl_root_installed(openssl_dir: &Path) -> Result<(), String> {
    use std::process::Command;

    let certutil = Path::new(&env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string()))
        .join("System32")
        .join("certutil.exe");
    if !certutil.exists() {
        return Err(format!(
            "Windows certutil.exe was not found at {}.",
            certutil.display()
        ));
    }

    let root_cert = openssl_dir.join("ca").join("rootCA.pem");
    if !root_cert.exists() {
        return Err(format!(
            "HocDev root certificate not found at {}.",
            root_cert.display()
        ));
    }

    let openssl_exe = openssl_executable(openssl_dir)?;
    let fp_output = Command::new(&openssl_exe)
        .args(["x509", "-in"])
        .arg(&root_cert)
        .args(["-noout", "-fingerprint", "-sha1"])
        .creation_flags(WINDOWS_CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to get root CA fingerprint: {e}"))?;

    let mut target_hash = String::new();
    if fp_output.status.success() {
        let fp_str = String::from_utf8_lossy(&fp_output.stdout);
        let hash_part = fp_str.split('=').nth(1).unwrap_or("").trim();
        target_hash = hash_part.replace(":", "").to_lowercase();
    }

    let is_installed = target_hash.len() > 10 && {
        let mut found = false;
        for args in [vec!["-user", "-store", "Root"], vec!["-store", "Root"]] {
            if let Ok(output) = Command::new(&certutil)
                .args(&args)
                .creation_flags(WINDOWS_CREATE_NO_WINDOW)
                .output()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let clean_stdout = stdout.replace(" ", "").replace("\r", "").replace("\n", "").to_lowercase();
                if clean_stdout.contains(&target_hash) {
                    found = true;
                    break;
                }
            }
        }
        found
    };

    if is_installed {
        return Ok(());
    }

    let output = Command::new(&certutil)
        .args(["-user", "-addstore", "Root"])
        .arg(&root_cert)
        .creation_flags(WINDOWS_CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to install HocDev root certificate: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let details = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(format!(
            "Windows trust-store install failed for HocDev root certificate: {details}"
        ));
    }

    Ok(())
}

#[cfg(windows)]
fn ssl_cert_issued_by_local_ca(openssl_dir: &Path, cert_path: &Path) -> Result<bool, String> {
    const MINI_PANEL_INTERMEDIATE_SUBJECT: &str = "HocDev Private CA Intermediate";

    if !cert_path.exists() {
        return Ok(false);
    }

    let openssl_exe = openssl_executable(openssl_dir)?;

    let output = std::process::Command::new(&openssl_exe)
        .args(["x509", "-in"])
        .arg(cert_path)
        .args(["-noout", "-issuer"])
        .creation_flags(WINDOWS_CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to inspect existing SSL certificate: {e}"))?;
    if !output.status.success() {
        return Ok(false);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.contains(MINI_PANEL_INTERMEDIATE_SUBJECT))
}

#[cfg(windows)]
fn apply_ssl_for_domain(domain: &str) -> Result<(), String> {
    let openssl_dir = bundled_openssl_dir()?;
    let openssl_exe = openssl_executable(&openssl_dir)?;
    ensure_local_ssl_ca_exists(&openssl_dir)?;
    ensure_windows_ssl_root_installed(&openssl_dir)?;

    let cert_dir =
        ssl_cert_dir().ok_or_else(|| "Unable to resolve SSL certificate directory".to_string())?;
    fs::create_dir_all(&cert_dir)
        .map_err(|e| format!("Failed to create SSL cert directory: {e}"))?;

    let cert_path = cert_dir.join(format!("{domain}.crt"));
    let key_path = cert_dir.join(format!("{domain}.key"));
    if cert_path.exists() && key_path.exists() {
        if ssl_cert_issued_by_local_ca(&openssl_dir, &cert_path)? {
            return Ok(());
        }
        let _ = fs::remove_file(&cert_path);
        let _ = fs::remove_file(&key_path);
    }

    let intermediate_cert = openssl_dir.join("ca").join("intermediateCA.pem");
    let intermediate_key = openssl_dir.join("ca").join("intermediateCA-key.pem");
    let intermediate_serial = openssl_dir.join("ca").join("intermediateCA.srl");
    let site_config_path = write_temp_ssl_file(
        "site-cert",
        ".cnf",
        &site_certificate_openssl_config(domain),
    )?;
    let csr_path = cert_dir.join(format!("{domain}.csr"));
    let temp_cert_path = cert_dir.join(format!("{domain}-temp.crt"));

    let result = (|| -> Result<(), String> {
        let mut key_command = std::process::Command::new(&openssl_exe);
        key_command.args(["ecparam", "-name", "secp384r1", "-genkey", "-noout", "-out"]);
        key_command.arg(&key_path);
        run_checked_command("OpenSSL site key generation", &mut key_command)?;

        let mut csr_command = std::process::Command::new(&openssl_exe);
        csr_command.args(["req", "-new", "-key"]);
        csr_command.arg(&key_path);
        csr_command.args(["-out"]);
        csr_command.arg(&csr_path);
        csr_command.args(["-config"]);
        csr_command.arg(&site_config_path);
        run_checked_command("OpenSSL site CSR generation", &mut csr_command)?;

        let mut cert_command = std::process::Command::new(&openssl_exe);
        cert_command.args(["x509", "-req", "-in"]);
        cert_command.arg(&csr_path);
        cert_command.args(["-CA"]);
        cert_command.arg(&intermediate_cert);
        cert_command.args(["-CAkey"]);
        cert_command.arg(&intermediate_key);
        cert_command.args(["-CAserial"]);
        cert_command.arg(&intermediate_serial);
        cert_command.args(["-CAcreateserial", "-out"]);
        cert_command.arg(&temp_cert_path);
        cert_command.args([
            "-days",
            "800",
            "-sha384",
            "-extensions",
            "v3_req",
            "-extfile",
        ]);
        cert_command.arg(&site_config_path);
        run_checked_command("OpenSSL site certificate signing", &mut cert_command)?;

        let mut fullchain = fs::read(&temp_cert_path)
            .map_err(|e| format!("Failed to read generated site certificate: {e}"))?;
        fullchain.extend(
            fs::read(&intermediate_cert)
                .map_err(|e| format!("Failed to read intermediate certificate chain: {e}"))?,
        );
        fs::write(&cert_path, fullchain)
            .map_err(|e| format!("Failed to write site certificate fullchain: {e}"))?;

        Ok(())
    })();

    remove_file_if_exists(&csr_path);
    remove_file_if_exists(&temp_cert_path);
    remove_file_if_exists(&site_config_path);
    if result.is_err() {
        remove_file_if_exists(&cert_path);
        remove_file_if_exists(&key_path);
    }
    result?;

    if !cert_path.exists() || !key_path.exists() {
        return Err(format!(
            "OpenSSL finished but the certificate files were not created for domain {domain}."
        ));
    }

    eprintln!(
        "SSL cert generated for {domain} using bundled OpenSSL: cert={}, key={}",
        cert_path.display(),
        key_path.display()
    );
    Ok(())
}

#[cfg(not(windows))]
fn apply_ssl_for_domain(_domain: &str) -> Result<(), String> {
    Err("SSL generation is only supported on Windows in this version.".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        default_create_index_php, ensure_site_index_php, hosts_content_with_domain,
        normalize_requested_website_domain,
    };
    use std::{env, fs, process, time::SystemTime};

    #[test]
    fn normalize_requested_website_domain_adds_test_suffix() {
        assert_eq!(
            normalize_requested_website_domain("DemoSite").unwrap(),
            "demosite.test"
        );
        assert_eq!(
            normalize_requested_website_domain("demo.test").unwrap(),
            "demo.test"
        );
    }

    #[test]
    fn hosts_content_with_domain_appends_only_missing_domain() {
        let original = "127.0.0.1 localhost\r\n# comment";
        let updated = hosts_content_with_domain(original, "demo.test").unwrap();

        assert!(updated.contains("127.0.0.1 localhost"));
        assert!(updated.contains("127.0.0.1 demo.test"));
        assert!(hosts_content_with_domain(&updated, "demo.test").is_none());
    }

    #[test]
    fn website_create_defaults_to_index_php() {
        assert!(default_create_index_php());
    }

    #[test]
    fn ensure_site_index_php_removes_generated_index_html_for_new_site() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let site_path =
            env::temp_dir().join(format!("MinPanel-website-test-{}-{unique}", process::id()));
        fs::create_dir_all(&site_path).unwrap();
        fs::write(site_path.join("index.html"), "legacy").unwrap();

        ensure_site_index_php(&site_path, false).unwrap();

        assert!(site_path.join("index.php").exists());
        assert!(!site_path.join("index.html").exists());

        let _ = fs::remove_dir_all(site_path);
    }
}

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process,
};

fn main() {
    let mut args = env::args_os().skip(1);
    let source = args.next();
    let target = args.next().filter(|value| !value.is_empty());
    let status = args
        .next()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);

    if let Some(path) = status.as_deref() {
        let _ = fs::remove_file(path);
    }

    let Some(source) = source.filter(|value| !value.is_empty()) else {
        fail(
            status.as_deref(),
            "Usage: update-hosts.exe <source-file> [target-file] [status-file]",
        );
    };

    let source = PathBuf::from(source);
    let target = target
        .map(PathBuf::from)
        .unwrap_or_else(default_windows_hosts_path);

    if !source.exists() {
        fail(
            status.as_deref(),
            &format!("Hosts update source not found: {}", source.display()),
        );
    }

    if let Err(error) = fs::copy(&source, &target) {
        fail(
            status.as_deref(),
            &format!(
                "Failed to update hosts file: {} ({error})",
                target.display()
            ),
        );
    }

    write_status(status.as_deref(), "ok");
}

#[cfg(windows)]
fn default_windows_hosts_path() -> PathBuf {
    let system_root = env::var_os("SystemRoot")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from(r"C:\Windows"));
    PathBuf::from(system_root)
        .join("System32")
        .join("drivers")
        .join("etc")
        .join("hosts")
}

#[cfg(not(windows))]
fn default_windows_hosts_path() -> PathBuf {
    PathBuf::from("/etc/hosts")
}

fn fail(status: Option<&Path>, message: &str) -> ! {
    write_status(status, message);
    eprintln!("{message}");
    process::exit(1);
}

fn write_status(status: Option<&Path>, message: &str) {
    if let Some(path) = status {
        let _ = fs::write(path, message);
    }
}

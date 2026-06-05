#[cfg(windows)]
fn main() {
    let icon_path = "assets/app.ico";
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set");
    let icon_source = std::path::Path::new(&manifest_dir).join(icon_path);
    let data_path = "data";
    let data_source = std::path::Path::new(&manifest_dir).join(data_path);

    println!("cargo:rerun-if-changed={icon_path}");
    println!("cargo:rerun-if-changed={data_path}");
    emit_rerun_if_changed_recursive(&data_source);

    if !icon_source.exists() {
        panic!("Windows icon resource not found at {icon_path}");
    }
    if !data_source.exists() {
        panic!("Windows data directory not found at {data_path}");
    }

    let mut resource = winres::WindowsResource::new();
    resource.set_icon(icon_source.to_string_lossy().as_ref());

    if let Err(error) = resource.compile() {
        panic!("failed to compile Windows resources: {error}");
    }

    sync_data_directory(&data_source);
}

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn sync_data_directory(source: &std::path::Path) {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is not set");
    let profile_dir = std::path::Path::new(&out_dir)
        .ancestors()
        .nth(3)
        .expect("failed to resolve Cargo profile directory");
    let target_dir = profile_dir.join("data");

    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir).unwrap_or_else(|error| {
            panic!(
                "failed to clear target data directory {}: {error}",
                target_dir.display()
            )
        });
    }

    copy_dir_recursive(source, &target_dir, source);
}

#[cfg(windows)]
fn copy_dir_recursive(
    source: &std::path::Path,
    target: &std::path::Path,
    data_root: &std::path::Path,
) {
    std::fs::create_dir_all(target).unwrap_or_else(|error| {
        panic!(
            "failed to create target directory {}: {error}",
            target.display()
        )
    });

    let entries = std::fs::read_dir(source).unwrap_or_else(|error| {
        panic!(
            "failed to read source directory {}: {error}",
            source.display()
        )
    });

    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!(
                "failed to iterate source directory {}: {error}",
                source.display()
            )
        });
        let entry_path = entry.path();
        let target_path = target.join(entry.file_name());

        if is_embedded_shared_ui_path(data_root, &entry_path) {
            continue;
        }

        if entry_path.is_dir() {
            copy_dir_recursive(&entry_path, &target_path, data_root);
        } else {
            std::fs::copy(&entry_path, &target_path).unwrap_or_else(|error| {
                panic!(
                    "failed to copy {} to {}: {error}",
                    entry_path.display(),
                    target_path.display()
                )
            });
        }
    }
}

#[cfg(windows)]
fn is_embedded_shared_ui_path(data_root: &std::path::Path, path: &std::path::Path) -> bool {
    path.strip_prefix(data_root)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| component.as_os_str() == std::ffi::OsStr::new("ui"))
}

#[cfg(windows)]
fn emit_rerun_if_changed_recursive(path: &std::path::Path) {
    if !path.exists() {
        return;
    }

    println!("cargo:rerun-if-changed={}", path.display());

    if path.is_dir() {
        let entries = std::fs::read_dir(path)
            .unwrap_or_else(|error| panic!("failed to read directory {}: {error}", path.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!("failed to iterate directory {}: {error}", path.display())
            });
            emit_rerun_if_changed_recursive(&entry.path());
        }
    }
}

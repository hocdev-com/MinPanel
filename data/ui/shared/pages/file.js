(() => {
  const app = window.MinPanel;
  if (!app?.addPageInitializer) return;

  const state = {
    root: "",
    current: "",
    parent: "",
    entries: [],
    search: "",
    selected: new Set(),
    uploadItems: [],
    uploadBusy: false,
    loading: false,
    error: "",
  };

  function html(text) {
    if (typeof escapeHtml === "function") return escapeHtml(text);
    const div = document.createElement("div");
    div.textContent = text ?? "";
    return div.innerHTML;
  }

  function bytes(value) {
    if (typeof formatBytes === "function") return formatBytes(value);
    const number = Number(value) || 0;
    if (number <= 0) return "0 B";
    const units = ["B", "KB", "MB", "GB", "TB"];
    const index = Math.min(Math.floor(Math.log(number) / Math.log(1024)), units.length - 1);
    const size = number / 1024 ** index;
    return `${size.toFixed(size >= 100 || index === 0 ? 0 : 1)} ${units[index]}`;
  }

  function formatTime(ms) {
    const value = Number(ms) || 0;
    if (!value) return "--";
    const date = new Date(value);
    const pad = (part) => String(part).padStart(2, "0");
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
  }

  function separatorFor(path) {
    return String(path || "").includes("\\") ? "\\" : "/";
  }

  function baseName(path) {
    return String(path || "")
      .split(/[\\/]+/)
      .filter(Boolean)
      .at(-1) || "";
  }

  function managedRootLabel() {
    const name = baseName(state.root);
    return name || "www";
  }

  function visibleColumnCount() {
    if (window.matchMedia?.("(max-width: 560px)").matches) return 3;
    if (window.matchMedia?.("(max-width: 760px)").matches) return 4;
    if (window.matchMedia?.("(max-width: 1100px)").matches) return 6;
    return 8;
  }

  function joinPath(base, name) {
    const separator = separatorFor(base);
    return `${String(base || "").replace(/[\\/]+$/, "")}${separator}${name}`;
  }

  function relativeParts() {
    if (!state.root || !state.current) return [];
    const root = state.root.toLowerCase();
    const current = state.current.toLowerCase();
    if (!current.startsWith(root)) return [];
    return state.current
      .slice(state.root.length)
      .replace(/^[\\/]+/, "")
      .split(/[\\/]+/)
      .filter(Boolean);
  }

  function buildPathFromParts(parts) {
    return parts.reduce((path, part) => joinPath(path, part), state.root);
  }

  function displayCurrentPath() {
    return `/${[managedRootLabel(), ...relativeParts()].join("/")}`;
  }

  function uploadRelativePath(file) {
    return file.webkitRelativePath || file.name;
  }

  function fileToBase64(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.addEventListener("load", () => {
        const result = String(reader.result || "");
        resolve(result.includes(",") ? result.split(",").pop() : result);
      });
      reader.addEventListener("error", () => reject(reader.error || new Error("Failed to read file")));
      reader.readAsDataURL(file);
    });
  }

  function getRemark(entry) {
    if (entry.remark) return entry.remark;
    if (entry.name === ".htaccess") return "Apache user configuration file (static)";
    if (entry.name === ".user.ini") return "PHP user configuration file (anti-cross-site)";
    return "";
  }

  function fileIconClass(entry) {
    if (entry.kind === "directory") return "folder";
    if (["html", "htm"].includes(entry.extension)) return "html";
    if (["css", "js", "json", "ini", "conf", "txt", "md"].includes(entry.extension)) return "text";
    if (["jpg", "jpeg", "png", "gif", "webp", "svg"].includes(entry.extension)) return "image";
    return "file";
  }

  function renderBreadcrumbs() {
    const host = document.getElementById("file-breadcrumbs");
    const back = document.getElementById("file-back-button");
    if (back) back.disabled = !state.parent;
    if (!host) return;

    const parts = relativeParts();
    const crumbs = [
      { label: "Root directory", path: state.root },
      { label: managedRootLabel(), path: state.root },
    ];
    parts.forEach((part, index) => {
      crumbs.push({ label: part, path: buildPathFromParts(parts.slice(0, index + 1)) });
    });

    host.innerHTML = crumbs
      .map((crumb, index) => `
        <button class="file-crumb" type="button" data-file-path="${html(crumb.path)}">
          ${html(crumb.label)}
        </button>
        ${index < crumbs.length - 1 ? '<span class="file-crumb-separator" aria-hidden="true">&rsaquo;</span>' : ""}
      `)
      .join("");
  }

  function renderRows() {
    const body = document.getElementById("file-table-body");
    const checkAll = document.getElementById("file-check-all");
    if (!body) return;

    if (state.loading) {
      body.innerHTML = `<tr><td colspan="${visibleColumnCount()}" class="file-empty-cell">Loading files...</td></tr>`;
      return;
    }

    if (state.error) {
      body.innerHTML = `<tr><td colspan="${visibleColumnCount()}" class="file-empty-cell is-error">${html(state.error)}</td></tr>`;
      return;
    }

    if (!state.entries.length) {
      body.innerHTML = `<tr><td colspan="${visibleColumnCount()}" class="file-empty-cell">No files in this directory.</td></tr>`;
      if (checkAll) checkAll.checked = false;
      return;
    }

    body.innerHTML = state.entries
      .map((entry) => {
        const selected = state.selected.has(entry.path);
        const isDirectory = entry.kind === "directory";
        const size = isDirectory ? '<button class="file-size-calculate" type="button" data-file-open="' + html(entry.path) + '">Calculate</button>' : html(bytes(entry.size));
        const primaryAction = isDirectory ? "" : `<button type="button" data-file-primary="${html(entry.path)}">Download</button>`;
        return `
          <tr data-file-entry="${html(entry.path)}" data-file-kind="${html(entry.kind)}">
            <td class="file-check-col"><input type="checkbox" data-file-select="${html(entry.path)}" ${selected ? "checked" : ""} aria-label="Select ${html(entry.name)}" /></td>
            <td class="file-name-col">
              <button class="file-name-button" type="button" data-file-open="${html(entry.path)}">
                <span class="file-entry-icon ${fileIconClass(entry)}" aria-hidden="true"></span>
                <span>${html(entry.name)}</span>
              </button>
            </td>
            <td class="file-protect-col"><span class="file-protect-state">${entry.protected ? "Protected" : "Unprotected"}</span></td>
            <td class="file-owner-col">${html(entry.permissions || "644/www")}</td>
            <td class="file-size-col">${size}</td>
            <td class="file-time-col">${html(formatTime(entry.modified_ms))}</td>
            <td class="file-remark-col">${html(getRemark(entry))}</td>
            <td class="file-operate-col">${primaryAction}</td>
          </tr>
        `;
      })
      .join("");

    if (checkAll) {
      checkAll.checked = state.entries.length > 0 && state.entries.every((entry) => state.selected.has(entry.path));
    }
  }

  function renderMeta(data = {}) {
    const label = document.getElementById("file-tab-label");
    const disk = document.getElementById("file-disk-label");
    const summary = document.getElementById("file-summary");
    const total = document.getElementById("file-total-count");

    const parts = relativeParts();
    if (label) label.textContent = parts.at(-1) || managedRootLabel();
    if (disk) disk.textContent = data.disk_label || "/ (Root)";
    if (summary) {
      summary.textContent = `${data.total_dirs || 0} directories, ${data.total_files || 0} files, file size`;
    }
    if (total) total.textContent = `Total ${(data.entries || state.entries).length}`;
  }

  function renderFileManager(data) {
    renderBreadcrumbs();
    renderRows();
    renderMeta(data);
    renderUploadMeta();
  }

  function renderUploadMeta() {
    const title = document.getElementById("file-upload-title");
    if (title) title.textContent = `Upload to [${displayCurrentPath()}]`;
  }

  function renderUploadList() {
    const list = document.getElementById("file-upload-list");
    const clear = document.getElementById("file-upload-clear");
    const start = document.getElementById("file-upload-start");
    if (clear) clear.disabled = !state.uploadItems.length || state.uploadBusy;
    if (start) {
      start.disabled = state.uploadBusy;
      start.textContent = state.uploadBusy ? "Uploading..." : "Start upload";
    }
    if (!list) return;
    if (!state.uploadItems.length) {
      list.hidden = true;
      list.innerHTML = "";
      return;
    }
    list.hidden = false;
    list.innerHTML = state.uploadItems
      .map((file) => `
        <div class="file-upload-list-row">
          <span>${html(uploadRelativePath(file))}</span>
          <span>${html(bytes(file.size || 0))}</span>
        </div>
      `)
      .join("");
  }

  function addUploadFiles(files) {
    state.uploadItems = Array.from(files || []).filter((file) => file?.name);
    renderUploadList();
  }

  function openUploadModal() {
    const modal = document.getElementById("file-upload-modal");
    const popover = document.getElementById("file-upload-popover");
    const button = document.getElementById("file-upload-button");
    if (popover) popover.hidden = true;
    button?.setAttribute("aria-expanded", "false");
    renderUploadMeta();
    renderUploadList();
    if (modal) modal.hidden = false;
  }

  function closeUploadModal() {
    const modal = document.getElementById("file-upload-modal");
    const selectPopover = document.getElementById("file-upload-select-popover");
    if (selectPopover) selectPopover.hidden = true;
    if (modal) modal.hidden = true;
  }

  async function startUpload() {
    if (!state.uploadItems.length) {
      document.getElementById("file-upload-input")?.click();
      return;
    }
    state.uploadBusy = true;
    renderUploadList();
    try {
      for (const file of state.uploadItems) {
        const content = await fileToBase64(file);
        const { response, body } = await fetchJsonWithTimeout(
          "/files/upload",
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              parent_path: state.current,
              relative_path: uploadRelativePath(file),
              content_base64: content,
            }),
          },
          30000,
        );
        if (!response.ok || !body.status) throw new Error(body.message || `HTTP ${response.status}`);
      }
      state.uploadItems = [];
      closeUploadModal();
      await loadFiles(state.current);
    } catch (error) {
      window.alert(error?.message || "Failed to upload files");
    } finally {
      state.uploadBusy = false;
      renderUploadList();
    }
  }

  async function loadFiles(path = state.current || "", options = {}) {
    state.loading = true;
    state.error = "";
    if (!options.keepSelection) state.selected.clear();
    renderFileManager();

    try {
      const { response, body } = await fetchJsonWithTimeout(
        "/files/list",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ path, search: state.search }),
        },
        10000,
      );
      if (!response.ok || !body.status) {
        throw new Error(body.message || `HTTP ${response.status}`);
      }
      state.root = body.root || "";
      state.current = body.current || "";
      state.parent = body.parent || "";
      state.entries = Array.isArray(body.entries) ? body.entries : [];
      state.loading = false;
      renderFileManager(body);
    } catch (error) {
      state.loading = false;
      state.error = error?.message || "Failed to load files";
      renderFileManager();
    }
  }

  async function createFileEntry(type) {
    const label = type === "directory" ? "directory" : "file";
    const name = window.prompt(`New ${label} name`);
    if (!name) return;
    const cleanName = name.trim();
    if (!cleanName || /[\\/]/.test(cleanName) || cleanName === "." || cleanName === "..") {
      window.alert("Name contains unsupported characters.");
      return;
    }

    try {
      if (type === "directory") {
        const { response, body } = await fetchJsonWithTimeout(
          "/files/directories/create",
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ parent_path: state.current, name: cleanName }),
          },
          10000,
        );
        if (!response.ok || !body.status) throw new Error(body.message || `HTTP ${response.status}`);
      } else {
        const { response, body } = await fetchJsonWithTimeout(
          "/files/write",
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ path: joinPath(state.current, cleanName), content: "" }),
          },
          10000,
        );
        if (!response.ok || String(body || "").startsWith("Error")) throw new Error(String(body || "Failed to create file"));
      }
      await loadFiles(state.current);
    } catch (error) {
      window.alert(error?.message || `Failed to create ${label}`);
    }
  }

  async function downloadFile(path) {
    try {
      const { response, body } = await fetchJsonWithTimeout(
        "/files/read",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ path }),
        },
        10000,
      );
      if (!response.ok || String(body || "").startsWith("Error")) {
        throw new Error(String(body || `HTTP ${response.status}`));
      }
      const entry = state.entries.find((item) => item.path === path);
      const blob = new Blob([body], { type: "text/plain;charset=utf-8" });
      const link = document.createElement("a");
      link.href = URL.createObjectURL(blob);
      link.download = entry?.name || "download.txt";
      document.body.appendChild(link);
      link.click();
      URL.revokeObjectURL(link.href);
      link.remove();
    } catch (error) {
      window.alert(error?.message || "Failed to download file");
    }
  }

  function bindFileManager() {
    const root = document.getElementById("files");
    if (!root || root.dataset.fileBound === "true") return;
    root.dataset.fileBound = "true";

    document.getElementById("file-refresh-button")?.addEventListener("click", () => loadFiles(state.current, { keepSelection: true }));
    document.getElementById("file-back-button")?.addEventListener("click", () => {
      if (state.parent) loadFiles(state.parent);
    });
    document.getElementById("file-search-button")?.addEventListener("click", () => {
      state.search = document.getElementById("file-search-input")?.value || "";
      loadFiles(state.current);
    });
    document.getElementById("file-search-input")?.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      state.search = event.currentTarget.value || "";
      loadFiles(state.current);
    });

    const newButton = document.getElementById("file-new-button");
    const newPopover = document.getElementById("file-new-popover");
    const uploadButton = document.getElementById("file-upload-button");
    const uploadPopover = document.getElementById("file-upload-popover");
    const uploadModal = document.getElementById("file-upload-modal");
    const uploadSelectButton = document.getElementById("file-upload-select-button");
    const uploadSelectPopover = document.getElementById("file-upload-select-popover");
    const uploadInput = document.getElementById("file-upload-input");
    const uploadFolderInput = document.getElementById("file-upload-folder-input");
    const uploadDrop = document.getElementById("file-upload-drop");
    uploadButton?.addEventListener("click", () => {
      if (!uploadPopover) return;
      const open = uploadPopover.hidden;
      uploadPopover.hidden = !open;
      uploadButton.setAttribute("aria-expanded", String(open));
    });
    uploadPopover?.addEventListener("click", (event) => {
      const button = event.target.closest("[data-file-upload-action]");
      if (!button || button.disabled) return;
      if (button.dataset.fileUploadAction === "upload") openUploadModal();
    });
    uploadSelectButton?.addEventListener("click", () => {
      if (!uploadSelectPopover) return;
      uploadSelectPopover.hidden = !uploadSelectPopover.hidden;
    });
    uploadSelectPopover?.addEventListener("click", (event) => {
      const button = event.target.closest("[data-file-upload-pick]");
      if (!button) return;
      uploadSelectPopover.hidden = true;
      if (button.dataset.fileUploadPick === "folder") uploadFolderInput?.click();
      else uploadInput?.click();
    });
    uploadInput?.addEventListener("change", (event) => addUploadFiles(event.target.files));
    uploadFolderInput?.addEventListener("change", (event) => addUploadFiles(event.target.files));
    uploadDrop?.addEventListener("dragover", (event) => {
      event.preventDefault();
      uploadDrop.classList.add("is-dragging");
    });
    uploadDrop?.addEventListener("dragleave", () => uploadDrop.classList.remove("is-dragging"));
    uploadDrop?.addEventListener("drop", (event) => {
      event.preventDefault();
      uploadDrop.classList.remove("is-dragging");
      addUploadFiles(event.dataTransfer?.files);
    });
    document.getElementById("file-upload-close")?.addEventListener("click", closeUploadModal);
    document.getElementById("file-upload-clear")?.addEventListener("click", () => {
      state.uploadItems = [];
      if (uploadInput) uploadInput.value = "";
      if (uploadFolderInput) uploadFolderInput.value = "";
      renderUploadList();
    });
    document.getElementById("file-upload-start")?.addEventListener("click", startUpload);
    uploadModal?.addEventListener("click", (event) => {
      if (event.target === uploadModal) closeUploadModal();
    });
    newButton?.addEventListener("click", () => {
      if (!newPopover) return;
      const open = newPopover.hidden;
      newPopover.hidden = !open;
      newButton.setAttribute("aria-expanded", String(open));
    });
    newPopover?.addEventListener("click", (event) => {
      const button = event.target.closest("[data-file-create]");
      if (!button) return;
      newPopover.hidden = true;
      newButton?.setAttribute("aria-expanded", "false");
      createFileEntry(button.dataset.fileCreate);
    });
    document.addEventListener("click", (event) => {
      if (!newPopover || newPopover.hidden) return;
      if (!event.target.closest(".file-new-menu")) {
        newPopover.hidden = true;
        newButton?.setAttribute("aria-expanded", "false");
      }
    });
    document.addEventListener("click", (event) => {
      if (uploadPopover && !uploadPopover.hidden && !event.target.closest(".file-upload-menu")) {
        uploadPopover.hidden = true;
        uploadButton?.setAttribute("aria-expanded", "false");
      }
      if (uploadSelectPopover && !uploadSelectPopover.hidden && !event.target.closest(".file-upload-select-menu")) {
        uploadSelectPopover.hidden = true;
      }
    });

    root.addEventListener("click", (event) => {
      const crumb = event.target.closest("[data-file-path]");
      if (crumb) {
        loadFiles(crumb.dataset.filePath);
        return;
      }

      const open = event.target.closest("[data-file-open]");
      if (open) {
        const entry = state.entries.find((item) => item.path === open.dataset.fileOpen);
        if (entry?.kind === "directory") loadFiles(entry.path);
        return;
      }

      const primary = event.target.closest("[data-file-primary]");
      if (primary) {
        const entry = state.entries.find((item) => item.path === primary.dataset.filePrimary);
        if (entry?.kind === "directory") {
          loadFiles(entry.path);
        } else if (entry) {
          downloadFile(entry.path);
        }
      }
    });

    root.addEventListener("change", (event) => {
      const select = event.target.closest("[data-file-select]");
      if (select) {
        if (select.checked) state.selected.add(select.dataset.fileSelect);
        else state.selected.delete(select.dataset.fileSelect);
        renderRows();
        return;
      }

      if (event.target.id === "file-check-all") {
        state.selected.clear();
        if (event.target.checked) {
          state.entries.forEach((entry) => state.selected.add(entry.path));
        }
        renderRows();
      }
    });
  }

  app.addPageInitializer("file-manager", () => {
    if (!document.getElementById("files")) return;
    bindFileManager();
    window.addEventListener("resize", () => renderRows());
    loadFiles("");
  });
})();

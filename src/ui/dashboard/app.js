let currentLogs = [];
let currentLogSnapshot = "--";
const DEFAULT_WEBSITE_DOMAIN_SUFFIX = ".test";
const WEBSITE_BATCH_OPTIONS = {
  "": "Please choose",
  backup: "Create backup",
  ssl: "Check SSL",
  waf: "Enable WAF",
  delete: "Delete site",
};

function createWebsiteDeleteDialogState(overrides = {}) {
  return {
    open: false,
    mode: "single",
    siteId: "",
    siteIds: [],
    siteName: "",
    deleteDocumentRoot: true,
    verifyLeft: 0,
    verifyRight: 0,
    verifyInput: "",
    error: "",
    ...overrides,
  };
}

const sidebarNavConfig = {
  Dashboard: { path: "/dashboard", section: "dashboard" },
  Website: { path: "/website", section: "website" },
  "App Store": { path: "/software", section: "software" },
  Traffic: { path: "/traffic", section: "traffic" },
  Disk: { path: "/disks", section: "disks" },
  Process: { path: "/processes", section: "processes" },
  "System API": { path: "/system" },
  "Process API": { path: "/process" },
  "Login API": { path: "/login" },
};

const trafficState = {
  labels: [],
  upload: [],
  download: [],
  previousSamples: {},
  currentSelection: "all",
  currentUnit: "kb",
  currentTab: "traffic",
  networks: [],
};

const websiteState = {
  items: [],
  project: "PHP Project",
  statusFilter: "all",
  search: "",
  page: 1,
  pageSize: 10,
  selected: new Set(),
  batchAction: "",
  batchMenuOpen: false,
  batchPending: false,
  phpRuntimes: [],
  websiteRoot: "",
  openMenuId: null,
  menuPosition: null,
  pendingActions: {},
  pendingDeleteId: null,
  deleteDialog: createWebsiteDeleteDialogState(),
};

const softwareState = {
  items: [],
  categories: [],
  category: "All",
  search: "",
  page: 1,
  pageSize: 5,
  pendingActions: {},
  optimisticStates: {},
  installModal: {
    open: false,
    title: "",
    versions: [],
    selectedVersionId: "",
  },
};

let dashboardRefreshPromise = null;

function hasPendingSoftwareActions() {
  return Object.keys(softwareState.pendingActions).length > 0;
}

async function fetchJsonWithTimeout(url, options = {}, timeoutMs = 10000) {
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, { ...options, signal: controller.signal });
    const body = await response.json().catch(() => ({ status: false }));
    return { response, body };
  } finally {
    window.clearTimeout(timeout);
  }
}

function normalizeDashboardPath(pathname) {
  if (!pathname || pathname === "/") return "/dashboard";
  if (pathname === "/overview") return "/website";
  return pathname;
}

function syncDashboardRoute() {
  const currentPath = normalizeDashboardPath(window.location.pathname);
  document.querySelectorAll(".menu a").forEach((link) => {
    link.classList.toggle("active", link.getAttribute("href") === currentPath);
  });

  const statusShell = document.querySelector(".status-shell");
  if (statusShell) {
    statusShell.hidden = currentPath === "/website";
  }

  const config = Object.values(sidebarNavConfig).find((entry) => entry.path === currentPath);
  if (!config || !config.section) return;

  const target = document.getElementById(config.section);
  if (!target) return;

  requestAnimationFrame(() => {
    target.scrollIntoView({ block: "start" });
  });
}

function formatBytes(bytes) {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exponent;
  return `${value.toFixed(value >= 100 || exponent === 0 ? 0 : 1)} ${units[exponent]}`;
}

function formatPercent(value) {
  return `${Number(value || 0).toFixed(1)}%`;
}

function formatAaPanelMegabytes(bytes) {
  return Math.max(0, Math.round((bytes || 0) / (1024 * 1024)));
}

function getAaPanelStatus(percent) {
  if (percent >= 90) return "Running blocked";
  if (percent >= 80) return "Running slowly";
  if (percent >= 70) return "Running normally";
  return "Running smoothly";
}

function formatAaPanelUptime(seconds) {
  const totalSeconds = Math.max(0, Number(seconds) || 0);
  const days = Math.floor(totalSeconds / 86400);
  return `${days} Day(s)`;
}

function formatLogStamp(date) {
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function setMeter(id, value) {
  const safeValue = Math.max(0, Math.min(100, value));
  const meter = document.getElementById(id);
  meter.style.setProperty("--progress", `${safeValue * 3.6}deg`);
}

function getNonLoopbackNetworks(networks) {
  if (!Array.isArray(networks)) return [];
  const filtered = networks.filter((entry) => entry && !/loopback|^lo$/i.test(entry.name));
  return filtered.length ? filtered : networks.filter(Boolean);
}

function getTrafficUnitDivisor(unit) {
  switch (unit) {
    case "mb":
      return 1024 * 1024;
    case "bytes":
      return 1;
    case "kb":
    default:
      return 1024;
  }
}

function getTrafficUnitLabel(unit) {
  switch (unit) {
    case "mb":
      return "MB";
    case "bytes":
      return "B";
    case "kb":
    default:
      return "KB";
  }
}

function formatTrafficSpeed(bytes, unit = trafficState.currentUnit) {
  if (!Number.isFinite(bytes) || bytes <= 0) return `0 ${getTrafficUnitLabel(unit)}`;
  const divisor = getTrafficUnitDivisor(unit);
  const value = bytes / divisor;
  const digits = value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits).replace(/\.0+$|(\.\d*[1-9])0+$/, "$1")} ${getTrafficUnitLabel(unit)}`;
}

function getSelectedTrafficSample(networks) {
  const available = getNonLoopbackNetworks(networks);
  if (!available.length) {
    return {
      key: trafficState.currentSelection,
      totalTransmitted: 0,
      totalReceived: 0,
    };
  }

  if (trafficState.currentSelection !== "all") {
    const selected = available.find((entry) => entry.name === trafficState.currentSelection);
    if (selected) {
      return {
        key: selected.name,
        totalTransmitted: selected.total_transmitted,
        totalReceived: selected.total_received,
      };
    }
  }

  return {
    key: "all",
    totalTransmitted: available.reduce((sum, entry) => sum + (entry.total_transmitted || 0), 0),
    totalReceived: available.reduce((sum, entry) => sum + (entry.total_received || 0), 0),
  };
}

function populateNetworkSelect(networks) {
  const select = document.getElementById("traffic-network-select");
  const available = getNonLoopbackNetworks(networks);
  const nextOptions = [{ value: "all", label: "Net: All" }].concat(
    available.map((entry) => ({ value: entry.name, label: `Net: ${entry.name}` })),
  );
  const currentMarkup = Array.from(select.options)
    .map((option) => `${option.value}:${option.textContent}`)
    .join("|");
  const nextMarkup = nextOptions.map((option) => `${option.value}:${option.label}`).join("|");

  if (currentMarkup !== nextMarkup) {
    select.innerHTML = nextOptions
      .map((option) => `<option value="${option.value}">${option.label}</option>`)
      .join("");
  }

  const hasSelection = nextOptions.some((option) => option.value === trafficState.currentSelection);
  if (!hasSelection) {
    trafficState.currentSelection = "all";
  }
  select.value = trafficState.currentSelection;
}

function setTrafficTab(tab) {
  trafficState.currentTab = tab;
  document.querySelectorAll("[data-traffic-tab]").forEach((button) => {
    const active = button.dataset.trafficTab === tab;
    button.classList.toggle("active", active);
    button.setAttribute("aria-selected", active ? "true" : "false");
  });
  document.querySelectorAll("[data-traffic-panel]").forEach((panel) => {
    panel.hidden = panel.dataset.trafficPanel !== tab;
  });
}

function softwareVisual(type, item = null) {
  const name = item && (item.name || item.title) ? String(item.name || item.title).toLowerCase() : "";
  if (name.includes("phpmyadmin")) return `<span class="soft-icon-phpmyadmin"></span>`;
  if (name.includes("mysql") || name.includes("mariadb")) return `<span class="soft-icon-mysql"></span>`;
  if (name.includes("pure-ftpd") || name.includes("pureftpd")) return `<span class="soft-icon-pureftpd"></span>`;
  if (name.includes("java")) return `<span class="soft-icon-java"></span>`;
  if (name.includes("docker")) return `<span class="soft-icon-docker"></span>`;
  if (name.includes("openlitespeed")) return `<span class="soft-icon-openlitespeed"></span>`;

  const customIcons = ["apache", "docker", "java", "mysql", "nginx", "openlitespeed", "php", "phpmyadmin", "pureftpd", "redis"];
  const lowerType = type ? String(type).toLowerCase() : "";
  if (customIcons.includes(lowerType)) {
    return `<span class="soft-icon-${lowerType}"></span>`;
  }
  switch (type) {
    case "dolphin":
      return '<svg viewBox="0 0 64 64" fill="none"><path d="M15 35c9-16 20-18 31-14-4 4-6 8-6 12 0 6 5 9 9 11-8 1-14-1-19-6-2 5-5 8-9 9 2-4 2-8 0-12-2 1-4 2-6 0z" stroke="#5a9fd7" stroke-width="3" stroke-linejoin="round"/></svg>';
    case "sail":
      return '<svg viewBox="0 0 64 64" fill="none"><path d="M15 44h34" stroke="#a78965" stroke-width="3" stroke-linecap="round"/><path d="M30 12v31" stroke="#9a7b4d" stroke-width="3"/><path d="M30 14 16 39h14z" fill="#e59b2f"/><path d="M32 16v25h16z" fill="#d9891e"/><path d="M15 44c4 5 10 7 17 7s13-2 17-7" stroke="#c3a27a" stroke-width="3" stroke-linecap="round"/></svg>';
    case "node":
      return '<svg viewBox="0 0 64 64" fill="none"><path d="M32 10 49 20v24L32 54 15 44V20z" stroke="#72bf44" stroke-width="3" stroke-linejoin="round"/><path d="M28 26v12c0 3 2 5 5 5s5-2 5-5V24" stroke="#72bf44" stroke-width="3" stroke-linecap="round"/><path d="M38 31c2-2 5-2 7 0" stroke="#72bf44" stroke-width="3" stroke-linecap="round"/></svg>';
    case "memcached":
      return '<svg viewBox="0 0 64 64" fill="none"><path d="M13 24c3-6 8-9 14-9 6 0 10 3 13 8 3-5 8-8 14-8 4 0 7 2 10 5-2 7-4 14-4 21 0 5 1 9 3 13-6 1-12-1-16-5-4 3-9 5-15 5-10 0-16-5-19-16-1-4-1-9 0-14z" fill="#1e88d6"/><path d="M24 35c2 2 4 3 7 3s5-1 7-3" stroke="#fff" stroke-width="3" stroke-linecap="round"/></svg>';
    case "waf":
      return '<svg viewBox="0 0 64 64" fill="none"><path d="M32 12 49 19v12c0 12-7 20-17 23-10-3-17-11-17-23V19z" stroke="#63b056" stroke-width="3" stroke-linejoin="round"/><path d="M22 31h20" stroke="#63b056" stroke-width="3"/><path d="M32 21v20" stroke="#63b056" stroke-width="3"/><path d="M18 43c8-5 20-5 28 0" stroke="#63b056" stroke-width="3" stroke-linecap="round"/></svg>';
    case "target":
      return '<svg viewBox="0 0 64 64" fill="none"><circle cx="32" cy="32" r="18" stroke="#1fa0ed" stroke-width="3"/><circle cx="32" cy="32" r="9" stroke="#1fa0ed" stroke-width="3"/><path d="M32 10v14M32 40v14M10 32h14M40 32h14" stroke="#1fa0ed" stroke-width="3" stroke-linecap="round"/></svg>';
    case "lock":
      return '<svg viewBox="0 0 64 64" fill="none"><rect x="16" y="20" width="32" height="28" rx="5" stroke="#b9bacb" stroke-width="3"/><path d="M23 20v-4c0-6 4-10 9-10 5 0 9 4 9 10v4" stroke="#b9bacb" stroke-width="3"/><path d="M32 31v8" stroke="#86c34a" stroke-width="3" stroke-linecap="round"/><circle cx="32" cy="29" r="2" fill="#86c34a"/></svg>';
    default:
      return '<span class="software-wordmark" style="color:#64748b">app</span>';
  }
}

function getSoftwareCategories() {
  const categories = ["All", "Installed"];
  softwareState.categories.forEach((item) => {
    if (item && item.title && !categories.includes(item.title)) {
      categories.push(item.title);
    }
  });
  getSoftwareDisplayItems().forEach((item) => {
    if (item.category && !categories.includes(item.category)) {
      categories.push(item.category);
    }
  });
  return categories;
}

function softwarePendingLabel(action) {
  switch (action) {
    case "install":
      return "Installing...";
    case "uninstall":
      return "Uninstalling...";
    case "start":
      return "Starting...";
    case "stop":
      return "Stopping...";
    default:
      return "Working...";
  }
}

function getSoftwareDisplayItem(item) {
  const optimisticState = softwareState.optimisticStates[item.id];
  const pendingAction = softwareState.pendingActions[item.id];
  const next = optimisticState ? { ...item, ...optimisticState } : { ...item };

  if (pendingAction === "install") {
    next.actions = [softwarePendingLabel(pendingAction)];
  } else if (pendingAction === "uninstall") {
    next.installed = true;
    next.actions = [softwarePendingLabel(pendingAction)];
  } else if (pendingAction === "start") {
    next.installed = true;
  } else if (pendingAction === "stop") {
    next.installed = true;
  }

  next.pendingAction = pendingAction || "";
  return next;
}

function getSoftwareDisplayItems() {
  return softwareState.items.map((item) => getSoftwareDisplayItem(item));
}

function setSoftwareOptimisticState(id, action) {
  delete softwareState.pendingActions[id];
  const item = softwareState.items.find((entry) => entry.id == id);
  const runtimeName = String(item?.name || item?.id || "").toLowerCase();
  const autoStartOnInstall = ["apache", "php", "mysql"].includes(runtimeName);
  if (action === "install") {
    softwareState.optimisticStates[id] = { installed: true, actions: ["Uninstall"], status: autoStartOnInstall ? "running" : "stopped" };
    return;
  }
  if (action === "uninstall") {
    softwareState.optimisticStates[id] = { installed: false, actions: ["Install"], status: "stopped" };
    return;
  }
  if (action === "start") {
    softwareState.optimisticStates[id] = { installed: true, actions: ["Uninstall"], status: "running" };
    return;
  }
  if (action === "stop") {
    softwareState.optimisticStates[id] = { installed: true, actions: ["Uninstall"], status: "stopped" };
  }
}

function clearSoftwareOptimisticStateIfConfirmed(items) {
  items.forEach((item) => {
    if (softwareState.pendingActions[item.id]) return;
    const optimisticState = softwareState.optimisticStates[item.id];
    if (!optimisticState) return;

    const installedMatches = optimisticState.installed === undefined || item.installed === optimisticState.installed;
    const statusMatches = optimisticState.status === undefined || item.status === optimisticState.status;
    if (installedMatches && statusMatches) {
      delete softwareState.optimisticStates[item.id];
    }
  });
}

function getSoftwareView() {
  const query = softwareState.search;
  const allItems = getSoftwareDisplayItems();
  const filtered = allItems.filter((item) => {
    const matchesCategory = softwareState.category === "All"
      || (softwareState.category === "Installed" ? item.installed : item.category === softwareState.category);
    const haystack = `${item.title} ${item.version} ${item.developer} ${item.description} ${item.category}`.toLowerCase();
    const matchesSearch = !query || haystack.includes(query);
    return matchesCategory && matchesSearch;
  });

  // Grouping logic:
  // We group EVERYTHING by title to provide a unified family-based view.
  const groups = {};
  filtered.forEach((item) => {
    if (!groups[item.title]) {
      groups[item.title] = {
        title: item.title,
        items: [],
        representative: item // Use some heuristic later
      };
    }
    groups[item.title].items.push(item);
  });

  const resultItems = Object.values(groups).map(group => {
    const versions = group.items;
    
    // Check if any version in this group is currently pending
    const pendingVersion = versions.find(v => softwareState.pendingActions[v.id]);
    const pendingAction = pendingVersion ? softwareState.pendingActions[pendingVersion.id] : "";
    
    // Sort versions newest first
    versions.sort((a, b) => b.version.localeCompare(a.version, undefined, { numeric: true, sensitivity: "base" }));
    
    const countInstalled = versions.filter(v => v.installed).length;
    const countAvailable = versions.filter(v => !v.installed).length;
    
    const representative = versions[0]; // Newest is rep

    return {
      ...representative,
      isGrouped: versions.length > 1,
      versions: versions,
      pendingAction: pendingAction,
      countInstalled,
      countAvailable,
      // Group status: Running if ANY is running
      status: versions.some(v => v.status === "running") ? "running" : "stopped",
      // If any is installed, it's "installed" in the table's context for status column
      installed: versions.some(v => v.installed)
    };
  });

  const totalPages = Math.max(1, Math.ceil(resultItems.length / softwareState.pageSize));
  softwareState.page = Math.min(softwareState.page, totalPages);
  const start = (softwareState.page - 1) * softwareState.pageSize;
  const pageItems = resultItems.slice(start, start + softwareState.pageSize);

  return { filtered: resultItems, totalPages, pageItems };
}

function formatSoftwarePrice(price) {
  if (!price) return "Free";
  return `$${Number(price).toFixed(0)}`;
}

function softwareStatusIndicator(status, pendingAction = "") {
  if (pendingAction === "start" || pendingAction === "stop") {
    return '<svg viewBox="0 0 20 20"><circle cx="10" cy="10" r="6.5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-dasharray="10 5" fill="none"></circle><path d="M10 6.4v3.6l2.4 1.6" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"></path></svg>';
  }
  if (status === "running") {
    return '<svg viewBox="0 0 20 20"><circle cx="10" cy="10" r="7" fill="currentColor" fill-opacity="0.14" stroke="currentColor" stroke-width="1.3"></circle><path d="m7.4 10.2 1.7 1.8 3.5-3.9" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"></path></svg>';
  }
  return '<svg viewBox="0 0 20 20"><circle cx="10" cy="10" r="7" fill="currentColor" fill-opacity="0.12" stroke="currentColor" stroke-width="1.3"></circle><path d="M7 10h6" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"></path></svg>';
}

function renderSoftwareCategories() {
  const categoryList = document.getElementById("software-category-list");
  if (!categoryList) return;
  categoryList.innerHTML = getSoftwareCategories()
    .map((category) => {
      const active = category === softwareState.category ? " active" : "";
      return `<button class="software-category-chip${active}" type="button" data-software-category="${escapeHtml(category)}">${escapeHtml(category)}</button>`;
    })
    .join("");
}

function renderSoftwareRecently() {
  const recently = document.getElementById("software-recently");
  if (!recently) return;
  const items = getSoftwareDisplayItems().filter((item) => item.installed).slice(0, 4);
  recently.innerHTML = `
    <div class="software-recently-main">
      <div class="software-recently-title">Recently plugin:</div>
      <div class="software-recently-list">
        ${items.map((item) => `<span class="software-recently-pill">${escapeHtml(`${item.title} ${item.version}`)}</span>`).join("")}
      </div>
    </div>
    <button class="software-refresh-button software-recently-action" id="software-refresh-button" type="button">Update App List</button>
  `;
}

function renderDashboardSoftwareSummary() {
  const list = document.getElementById("software-list");
  if (!list) return;

  const displayItems = getSoftwareDisplayItems();
  const installedItems = displayItems.filter((item) => item.installed);
  const summaryItems = (installedItems.length ? installedItems : displayItems).slice(0, 8);

  if (!summaryItems.length) {
    list.innerHTML = '<div class="software-empty-state">No software detected yet.</div>';
    return;
  }

  list.innerHTML = summaryItems
    .map((item) => {
      const footer = item.installed
        ? `
          <div class="software-bottom">
            <span class="software-state${item.status === "running" ? "" : " is-stopped"}">
              ${escapeHtml(item.status === "running" ? "Running" : "Stopped")}
            </span>
          </div>
        `
        : `
          <div class="software-bottom">
            <div class="software-actions">
              ${item.actions
                .slice(0, 2)
                .map((action) => `<span class="software-action${action === "Buy now" ? " buy" : ""}">${escapeHtml(action)}</span>`)
                .join("")}
            </div>
          </div>
        `;

      return `
        <article class="software-item">
          <div class="software-visual" aria-hidden="true">${softwareVisual(item.visual, item)}</div>
          <div class="software-name">${escapeHtml(`${item.title} ${item.version}`.trim())}</div>
          ${footer}
        </article>
      `;
    })
    .join("");
}

function getSoftwareVisibleColumnCount() {
  const width = window.innerWidth || document.documentElement.clientWidth || 0;
  if (width <= 540) return 2;
  if (width <= 900) return 4;
  if (width <= 1180) return 5;
  if (width <= 1400) return 6;
  return 7;
}

function renderOverviewStats(data) {
  const siteCount = document.getElementById("overview-site-count");
  const ftpCount = document.getElementById("overview-ftp-count");
  const dbCount = document.getElementById("overview-db-count");
  const securityCount = document.getElementById("overview-security-count");

  if (siteCount) siteCount.textContent = String(data.site_count ?? 0);
  if (ftpCount) ftpCount.textContent = String(data.ftp_count ?? 0);
  if (dbCount) dbCount.textContent = String(data.database_count ?? 0);
  if (securityCount) securityCount.textContent = String(data.warning_count ?? 0);
}

function renderSoftwareList() {
  const tbody = document.getElementById("software-table-body");
  if (!tbody) return;

  renderSoftwareCategories();
  renderSoftwareRecently();
  const { filtered, totalPages, pageItems } = getSoftwareView();

  if (!pageItems.length) {
    tbody.innerHTML = `<tr class="software-empty"><td colspan="${getSoftwareVisibleColumnCount()}">No applications match the current filters.</td></tr>`;
  } else {
    tbody.innerHTML = pageItems
      .map((item) => {
        const priceClass = item.price ? " is-paid" : " is-free";
        const statusClass = item.status === "running" ? " is-running" : " is-stopped";
        const actionBusy = Boolean(item.pendingAction);
        const statusText = item.pendingAction === "start"
          ? "Starting..."
          : item.pendingAction === "stop"
            ? "Stopping..."
            : item.countInstalled > 0
              ? (item.status === "running" ? "Running" : "Stopped")
              : "--";
        const location = item.installed
          ? '<button class="software-location-button" type="button" aria-label="Open install path"><svg viewBox="0 0 20 20"><path d="M2.8 6.5h4l1.4 1.7h8.9v5.8a1.5 1.5 0 0 1-1.5 1.5H4.3a1.5 1.5 0 0 1-1.5-1.5z"></path><path d="M2.8 6.5V5.6a1.3 1.3 0 0 1 1.3-1.3h2.3l1.2 1.4h2.2"></path></svg></button>'
          : '<span class="software-location-empty">--</span>';
        
        const versionLabel = `${escapeHtml(item.title)} (` +
          item.versions.map(v => 
            `<span class="software-v-item ${v.installed ? 'is-installed' : 'is-available'}">${escapeHtml(v.version)}</span>`
          ).join(", ") +
          `)`;

        return `
          <tr>
            <td class="software-app-col">
              <div class="software-app-cell">
                <span class="software-app-icon" aria-hidden="true">${softwareVisual(item.visual, item)}</span>
                <span class="software-app-name">${versionLabel}</span>
              </div>
            </td>
            <td class="software-developer software-developer-col">${escapeHtml(item.developer)}</td>
            <td class="software-description software-description-col">${escapeHtml(item.description)}</td>
            <td class="software-price-col"><span class="software-price${priceClass}">${escapeHtml(formatSoftwarePrice(item.price))}</span></td>
            <td class="software-location-col">${location}</td>
            <td class="software-status-col">
              ${item.countInstalled > 0
                ? `<span class="software-status software-status-indicator${statusClass}${actionBusy ? " is-busy" : ""}" title="${escapeHtml(statusText)}" aria-label="${escapeHtml(statusText)}">
                    <span class="software-status-icon" aria-hidden="true">${softwareStatusIndicator(item.status, item.pendingAction)}</span>
                  </span>`
                : '<span class="software-status software-status-empty">--</span>'}
            </td>
            <td class="software-operate-col">
              <span class="software-operate-links">
                ${actionBusy 
                  ? `<button class="software-operate-link software-operate-button" type="button" disabled>${escapeHtml(item.pendingAction === 'install' ? 'Installing...' : 'Working...')}</button>`
                  : `<button class="software-operate-link software-operate-button" type="button" data-software-open-install="${escapeHtml(item.title)}">${item.countInstalled > 0 ? "Manage" : "Install"}</button>`
                }
              </span>
            </td>
          </tr>
        `;
      })
      .join("");
    tbody.querySelectorAll("[data-software-open-install]").forEach((btn) => {
      btn.onclick = () => openSoftwareInstallModal(btn.dataset.softwareOpenInstall);
    });
  }

  // Sync manager modal if it's currently open
  if (softwareState.installModal && softwareState.installModal.open) {
    const freshAll = getSoftwareDisplayItems();
    softwareState.installModal.versions = freshAll
      .filter(item => item.title === softwareState.installModal.title)
      .sort((a, b) => b.version.localeCompare(a.version, undefined, { numeric: true, sensitivity: "base" }));
    refreshManagerModal();
  }

  const start = filtered.length ? (softwareState.page - 1) * softwareState.pageSize + 1 : 0;
  const end = filtered.length ? Math.min(filtered.length, softwareState.page * softwareState.pageSize) : 0;
  document.getElementById("software-page-meta").textContent = filtered.length
    ? `Showing ${start}-${end} of ${filtered.length} apps`
    : "0 apps";
  document.getElementById("software-page-current").textContent = `${softwareState.page} / ${totalPages}`;
  document.getElementById("software-prev-page").disabled = softwareState.page <= 1;
  document.getElementById("software-next-page").disabled = softwareState.page >= totalPages;
}

function bindSoftwareControls() {
  const softwareSection = document.getElementById("software");
  const categoryList = document.getElementById("software-category-list");
  const searchInput = document.getElementById("software-search-input");
  const searchButton = document.getElementById("software-search-button");
  const prevButton = document.getElementById("software-prev-page");
  const nextButton = document.getElementById("software-next-page");
  if (!softwareSection || !categoryList || !searchInput || !searchButton || !prevButton || !nextButton) return;

  categoryList.addEventListener("click", (event) => {
    const button = event.target.closest("[data-software-category]");
    if (!button) return;
    softwareState.category = button.dataset.softwareCategory;
    softwareState.page = 1;
    renderSoftwareList();
  });

  const submitSearch = () => {
    softwareState.search = searchInput.value.trim().toLowerCase();
    softwareState.page = 1;
    renderSoftwareList();
  };

  searchInput.addEventListener("input", submitSearch);
  searchInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      submitSearch();
    }
  });
  searchButton.addEventListener("click", submitSearch);
  
  const installModal = document.getElementById("software-install-modal");
  const installConfirm = document.getElementById("software-install-confirm");
  const installVersion = document.getElementById("software-install-version-dropdown");
  const installClose = document.getElementById("software-install-close");

  if (installModal) {
    installModal.addEventListener("click", (event) => {
      if (event.target.hasAttribute("data-software-install-close")) {
        closeSoftwareInstallModal();
      }
    });
  }

  if (installClose) {
    installClose.addEventListener("click", closeSoftwareInstallModal);
  }

  if (installVersion) {
    installVersion.addEventListener("change", (event) => {
      const id = event.target.value;
      softwareState.installModal.selectedVersionId = id;
      
      // Update modal UI with newly selected version details
      const selectedItem = softwareState.installModal.versions.find(v => v.id == id);
      if (selectedItem) {
        const iconHost = document.getElementById("software-install-icon");
        const descHost = document.getElementById("software-install-description");
        const updateTimeHost = document.getElementById("software-install-update-time");
        
        if (iconHost) iconHost.innerHTML = softwareVisual(selectedItem.visual, selectedItem);
        if (updateTimeHost) updateTimeHost.textContent = `Update time: ${selectedItem.update_time || selectedItem.expire || "--"}`;
        if (descHost) {
          descHost.innerHTML = `
            <p>${escapeHtml(selectedItem.description)}</p>
            <ul>
              <li>If this plugin already exists, the file will be replaced!</li>
              <li>Please install the plugin extensions and dependencies manually, if they are not installed, the plugin will not work properly</li>
              <li>The installation process may take a few minutes, so please be patient!</li>
            </ul>
          `;
        }
      }
    });
  }

  if (installConfirm) {
    installConfirm.addEventListener("click", async () => {
      const id = softwareState.installModal.selectedVersionId;
      const action = installConfirm.dataset.softwareAction || "install";
      if (!id || !action || softwareState.pendingActions[id]) return;
      if (action === "install") {
        closeSoftwareInstallModal();
      }
      runSoftwareAction(id, action);
    });
  }

  document.getElementById("software-table-body").addEventListener("click", async (event) => {
    // Manage modal
    const openInstall = event.target.closest("[data-software-open-install]");
    if (openInstall && openInstall.dataset.softwareOpenInstall) {
      openSoftwareInstallModal(openInstall.dataset.softwareOpenInstall);
      return;
    }

    const button = event.target.closest("[data-software-action]");
    if (!button) return;

    const action = button.dataset.softwareAction;
    const id = button.dataset.softwareId;
    if (!id || softwareState.pendingActions[id]) return;

    runSoftwareAction(id, action);
  });

  softwareSection.addEventListener("click", async (event) => {
    const refreshButton = event.target.closest("#software-refresh-button");
    if (!refreshButton) return;
    const originalLabel = "Update App List";
    refreshButton.disabled = true;
    refreshButton.textContent = "Updating...";
    try {
      const response = await fetch("/software/refresh", { method: "POST" });
      const result = await response.json().catch(() => ({ status: false }));
      if (!response.ok || !result.status) {
        throw new Error(result.message || `HTTP ${response.status}`);
      }
      refreshButton.textContent = "Updated";
      await refreshDashboard();
    } catch (error) {
      refreshButton.textContent = "Retry failed";
    } finally {
      setTimeout(() => {
        refreshButton.disabled = false;
        refreshButton.textContent = originalLabel;
      }, 1200);
    }
  });

  prevButton.addEventListener("click", () => {
    softwareState.page = Math.max(1, softwareState.page - 1);
    renderSoftwareList();
  });

  nextButton.addEventListener("click", () => {
    const { totalPages } = getSoftwareView();
    softwareState.page = Math.min(totalPages, softwareState.page + 1);
    renderSoftwareList();
  });
}

async function runSoftwareAction(id, action) {
  if (!id || softwareState.pendingActions[id]) return;
  softwareState.pendingActions[id] = action;
  renderDashboardSoftwareSummary();
  renderSoftwareList();
  try {
    const timeoutMs = action === "start" || action === "stop" ? 35000 : 30000;
    const { response, body: result } = await fetchJsonWithTimeout(
      `/software/${action}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ id }),
      },
      timeoutMs,
    );
    if (!response.ok || !result.status) {
      throw new Error(result.message || `HTTP ${response.status}`);
    }
    setSoftwareOptimisticState(id, action);
    renderDashboardSoftwareSummary();
    renderSoftwareList();
    await refreshDashboard();
    delete softwareState.optimisticStates[id];
    renderDashboardSoftwareSummary();
    renderSoftwareList();
  } catch (error) {
    delete softwareState.pendingActions[id];
    delete softwareState.optimisticStates[id];
    const errorMessage = error?.name === "AbortError"
      ? "The status action took too long and was cancelled. Check the runtime process and try again."
      : error?.message;
    if (errorMessage) {
      window.alert(errorMessage);
    }
    try {
      await refreshDashboard();
    } catch {
      renderDashboardSoftwareSummary();
      renderSoftwareList();
    }
  }
}

function openSoftwareInstallModal(title) {
  const allItems = getSoftwareDisplayItems();
  const versions = allItems
    .filter(item => item.title === title)
    .sort((a, b) => b.version.localeCompare(a.version, undefined, { numeric: true, sensitivity: "base" }));
  
  if (!versions.length) return;

  const modal = document.getElementById("software-install-modal");
  const iconHost = document.getElementById("software-install-icon");
  const titleHost = document.getElementById("software-install-title");
  
  // Smart version selection: Prioritize running, then installed, then newest
  let selectedId = versions[0].id;
  const running = versions.find(v => v.status === "running");
  const installed = versions.find(v => v.installed);
  
  if (running) {
    selectedId = running.id;
  } else if (installed) {
    selectedId = installed.id;
  }

  softwareState.installModal = {
    open: true,
    title: title,
    versions: versions,
    selectedVersionId: selectedId,
  };

  const selectedItem = versions.find(v => v.id == selectedId) || versions[0];
  if (iconHost) iconHost.innerHTML = softwareVisual(selectedItem.visual, selectedItem);
  if (titleHost) titleHost.textContent = title;
  
  refreshManagerModal();

  if (modal) modal.hidden = false;
}

function refreshManagerModal() {
  const dropdown = document.getElementById("software-install-version-dropdown");
  const confirmBtn = document.getElementById("software-install-confirm");
  if (!dropdown || !confirmBtn) return;

  const { versions, selectedVersionId } = softwareState.installModal;
  
  // Populate dropdown with ALL versions (colored or marked)
  dropdown.innerHTML = versions.map(v => {
    const statusText = v.installed ? (v.status === "running" ? " [Running]" : " [Stopped]") : "";
    return `<option value="${v.id}" ${v.id == selectedVersionId ? "selected" : ""}>${v.version}${statusText}</option>`;
  }).join("");

  // Update logic when selection changes
  dropdown.onchange = (e) => {
    softwareState.installModal.selectedVersionId = e.target.value;
    updateManagerUIForSelection();
  };

  updateManagerUIForSelection();
}

function updateManagerUIForSelection() {
  const confirmBtn = document.getElementById("software-install-confirm");
  const uninstallBtn = document.getElementById("software-install-uninstall");
  const secondaryActions = document.getElementById("software-manager-actions-row");
  const { versions, selectedVersionId } = softwareState.installModal;
  
  const item = versions.find(v => v.id == selectedVersionId);
  if (!item) return;

  const isPending = !!softwareState.pendingActions[item.id];
  
  if (isPending) {
    confirmBtn.disabled = true;
    confirmBtn.textContent = softwarePendingLabel(softwareState.pendingActions[item.id]);
    confirmBtn.dataset.softwareAction = "";
    if (uninstallBtn) uninstallBtn.hidden = true;
    secondaryActions.innerHTML = "";
  } else if (!item.installed) {
    confirmBtn.disabled = false;
    confirmBtn.textContent = "Install Now";
    confirmBtn.className = "software-install-confirm is-install";
    confirmBtn.dataset.softwareAction = "install";
    if (uninstallBtn) uninstallBtn.hidden = true;
    secondaryActions.innerHTML = "";
  } else {
    const oppositeAction = item.status === "running" ? "stop" : "start";
    confirmBtn.disabled = false;
    confirmBtn.textContent = oppositeAction.charAt(0).toUpperCase() + oppositeAction.slice(1);
    confirmBtn.className = "software-install-confirm";
    confirmBtn.dataset.softwareAction = oppositeAction;

    if (uninstallBtn) {
      uninstallBtn.hidden = false;
      uninstallBtn.disabled = false;
      uninstallBtn.onclick = () => runSoftwareAction(item.id, "uninstall");
    }
    secondaryActions.innerHTML = "";
  }

  if (uninstallBtn && isPending) {
    uninstallBtn.disabled = true;
  }

  updateManagerDescription(item.id);
}

function updateManagerDescription(id) {
  const descHost = document.getElementById("software-install-description");
  const updateTimeHost = document.getElementById("software-install-update-time");
  if (!descHost) return;

  const item = softwareState.installModal.versions.find(v => v.id == id);
  if (!item) return;

  if (updateTimeHost) updateTimeHost.textContent = `Update time: ${item.update_time || item.expire || "--"}`;
  descHost.innerHTML = `
    <p>${escapeHtml(item.description)}</p>
    <ul>
      <li>If this plugin already exists, the file will be replaced!</li>
      <li>Please install the plugin extensions and dependencies manually, if they are not installed, the plugin will not work properly</li>
      <li>The installation process may take a few minutes, so please be patient!</li>
    </ul>
  `;
}

function closeSoftwareInstallModal() {
  const modal = document.getElementById("software-install-modal");
  if (modal) modal.hidden = true;
  if (softwareState.installModal) {
    softwareState.installModal.open = false;
  }
}

function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function getWebsiteView() {
  const search = websiteState.search;
  const filtered = websiteState.items.filter((item) => {
    const matchesProject = item.category === websiteState.project;
    const matchesStatus = websiteState.statusFilter === "all"
      || item.status === websiteState.statusFilter
      || (websiteState.statusFilter === "expired" && item.ssl_status === "Expired");
    const haystack = `${item.name} ${item.alias} ${item.category}`.toLowerCase();
    const matchesSearch = !search || haystack.includes(search);
    return matchesProject && matchesStatus && matchesSearch;
  });

  const totalPages = Math.max(1, Math.ceil(filtered.length / websiteState.pageSize));
  websiteState.page = Math.min(websiteState.page, totalPages);
  const start = (websiteState.page - 1) * websiteState.pageSize;
  const pageItems = filtered.slice(start, start + websiteState.pageSize);

  return { filtered, totalPages, pageItems };
}

function websiteSiteIcon(sslEnabled) {
  return `<span class="website-site-protocol ${sslEnabled ? "is-https" : "is-http"}">${sslEnabled ? "HTTPS" : "HTTP"}</span>`;
}

function websiteStatusIcon(status) {
  if (status === "running") {
    return '<svg viewBox="0 0 20 20"><circle cx="10" cy="10" r="6"></circle><path d="m8.4 7.2 4.7 2.8-4.7 2.8z"></path></svg>';
  }
  return '<svg viewBox="0 0 20 20"><circle cx="10" cy="10" r="6"></circle><path d="M8.2 7.1v5.8"></path><path d="M11.8 7.1v5.8"></path></svg>';
}

function websiteSslTone(status) {
  switch (status) {
    case "Valid":
      return "is-valid";
    case "Expired":
      return "is-expired";
    case "Invalid":
      return "is-invalid";
    default:
      return "is-none";
  }
}

function websiteQuickIcon(kind) {
  switch (kind) {
    case "folder":
      return '<svg viewBox="0 0 20 20"><path d="M2.8 6.5h4l1.4 1.7h8.9v5.8a1.5 1.5 0 0 1-1.5 1.5H4.3a1.5 1.5 0 0 1-1.5-1.5z"></path><path d="M2.8 6.5V5.6a1.3 1.3 0 0 1 1.3-1.3h2.3l1.2 1.4h2.2"></path></svg>';
    case "speed":
      return '<svg viewBox="0 0 20 20"><path d="M4 12a6 6 0 1 1 12 0"></path><path d="m10 10 3.4-2.8"></path><path d="M6.5 13.5h7"></path></svg>';
    default:
      return "";
  }
}

function websiteActionMenuIcon(kind) {
  switch (kind) {
    case "security":
      return '<svg viewBox="0 0 20 20"><path d="M10 3.2 15.8 5v4.7c0 3.4-2.1 5.7-5.8 7.1-3.7-1.4-5.8-3.7-5.8-7.1V5z"></path><path d="M10 7.4v3.5"></path><circle cx="10" cy="13.5" r="0.9"></circle></svg>';
    case "category":
      return '<svg viewBox="0 0 20 20"><path d="M4 4.5h5.1v5.1H4z"></path><path d="M10.9 4.5H16v5.1h-5.1z"></path><path d="M7.45 10.4 11 16H3.9z"></path></svg>';
    case "delete":
      return '<svg viewBox="0 0 20 20"><path d="M5.8 6.2h8.4"></path><path d="M7.2 6.2V4.6h5.6v1.6"></path><path d="M6.8 6.2v8.2a1 1 0 0 0 1 1h4.4a1 1 0 0 0 1-1V6.2"></path><path d="M8.8 8.5v4.4"></path><path d="M11.2 8.5v4.4"></path></svg>';
    default:
      return "";
  }
}

function formatWebsiteRuntimeLabel(runtime) {
  const value = String(runtime || "").trim();
  const match = value.match(/(\d+\.\d+)/);
  return match ? match[1] : value || "PHP";
}

function renderWebsitePhpSelect(item) {
  return `<span class="website-quick-runtime">${escapeHtml(formatWebsiteRuntimeLabel(item.runtime))}</span>`;
}

function isWebsiteDeleteVerificationValid() {
  const { verifyLeft, verifyRight, verifyInput } = websiteState.deleteDialog;
  return Number(verifyInput.trim()) === verifyLeft + verifyRight;
}

function renderWebsiteDeleteModal() {
  const modal = document.getElementById("website-delete-modal");
  const title = document.getElementById("website-delete-title");
  const warningTitle = document.getElementById("website-delete-warning-title");
  const documentRoot = document.getElementById("website-delete-document-root");
  const expression = document.getElementById("website-delete-verify-expression");
  const input = document.getElementById("website-delete-verify-input");
  const confirmButton = document.getElementById("website-delete-confirm");
  const cancelButton = document.getElementById("website-delete-cancel");
  const closeButton = document.getElementById("website-delete-close");
  const error = document.getElementById("website-delete-error");
  if (!modal || !title || !warningTitle || !documentRoot || !expression || !input || !confirmButton || !cancelButton || !closeButton || !error) return;

  const {
    open,
    mode,
    siteName,
    siteIds,
    deleteDocumentRoot,
    verifyLeft,
    verifyRight,
    verifyInput,
    error: errorMessage,
  } = websiteState.deleteDialog;
  const deleteCount = mode === "batch" ? siteIds.length : 1;
  const isBatch = mode === "batch" && deleteCount > 1;
  modal.hidden = !open;
  title.textContent = isBatch
    ? `Delete ${deleteCount} sites`
    : `Delete site [${siteName || "--"}]`;
  warningTitle.textContent = isBatch
    ? `This will delete ${deleteCount} selected website profiles.`
    : "This will delete the selected website profile.";
  documentRoot.checked = deleteDocumentRoot;
  documentRoot.disabled = true;
  expression.textContent = `${verifyLeft} + ${verifyRight} =`;
  input.value = verifyInput;
  input.disabled = Boolean(websiteState.pendingDeleteId);
  confirmButton.disabled = Boolean(websiteState.pendingDeleteId) || !isWebsiteDeleteVerificationValid();
  confirmButton.textContent = websiteState.pendingDeleteId ? "Deleting..." : "Confirm";
  cancelButton.disabled = Boolean(websiteState.pendingDeleteId);
  closeButton.disabled = Boolean(websiteState.pendingDeleteId);
  error.hidden = !errorMessage;
  error.textContent = errorMessage || "";

  if (open && !websiteState.pendingDeleteId) {
    requestAnimationFrame(() => input.focus());
  }
}

function openWebsiteDeleteModal(websiteId) {
  const item = websiteState.items.find((entry) => entry.id === websiteId);
  if (!item || websiteState.pendingDeleteId === websiteId) return;

  websiteState.deleteDialog = createWebsiteDeleteDialogState({
    open: true,
    mode: "single",
    siteId: websiteId,
    siteIds: [websiteId],
    siteName: item.name,
    verifyLeft: Math.floor(Math.random() * 9) + 1,
    verifyRight: Math.floor(Math.random() * 9) + 1,
  });
  websiteState.batchMenuOpen = false;
  websiteState.openMenuId = null;
  websiteState.menuPosition = null;
  renderWebsiteTable();
}

function openWebsiteBatchDeleteModal(siteIds) {
  const resolvedIds = [...new Set(siteIds.filter(Boolean))];
  if (!resolvedIds.length || websiteState.pendingDeleteId) return;

  websiteState.deleteDialog = createWebsiteDeleteDialogState({
    open: true,
    mode: "batch",
    siteIds: resolvedIds,
    siteName: `${resolvedIds.length} selected sites`,
    verifyLeft: Math.floor(Math.random() * 9) + 1,
    verifyRight: Math.floor(Math.random() * 9) + 1,
  });
  websiteState.batchMenuOpen = false;
  renderWebsiteTable();
}

function closeWebsiteDeleteModal(force = false) {
  if (websiteState.pendingDeleteId && !force) return;
  websiteState.deleteDialog = createWebsiteDeleteDialogState();
  renderWebsiteDeleteModal();
}

async function deleteWebsiteSite() {
  const { mode, siteId, siteIds, deleteDocumentRoot } = websiteState.deleteDialog;
  const deleteTargets = mode === "batch" ? siteIds.filter(Boolean) : [siteId].filter(Boolean);
  if (!deleteTargets.length || websiteState.pendingDeleteId || !isWebsiteDeleteVerificationValid()) return;

  websiteState.pendingDeleteId = mode === "batch" ? "__batch__" : deleteTargets[0];
  websiteState.deleteDialog.error = "";
  renderWebsiteDeleteModal();

  try {
    for (const targetId of deleteTargets) {
      const { response, body } = await fetchJsonWithTimeout(
        "/website/delete",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            site_id: targetId,
            delete_document_root: deleteDocumentRoot,
          }),
        },
        15000,
      );
      if (!response.ok || !body.status) {
        throw new Error(body.message || "Delete website failed");
      }
    }

    const deletedIds = new Set(deleteTargets);
    websiteState.items = websiteState.items.filter((entry) => !deletedIds.has(entry.id));
    deletedIds.forEach((id) => websiteState.selected.delete(id));
    if (websiteState.batchAction === "delete") {
      websiteState.batchAction = "";
    }
    syncWebsiteProjectTabs();
    closeWebsiteDeleteModal(true);
    renderWebsiteTable();
  } catch (error) {
    websiteState.deleteDialog.error = error?.message || "Delete website failed";
    renderWebsiteDeleteModal();
  } finally {
    websiteState.pendingDeleteId = null;
    renderWebsiteDeleteModal();
    renderWebsiteTable();
  }
}

async function runWebsiteLifecycleAction(siteId, action) {
  if (!siteId || websiteState.pendingActions[siteId]) return;

  websiteState.pendingActions[siteId] = action;
  renderWebsiteTable();

  try {
    const { response, body } = await fetchJsonWithTimeout(
      `/website/${action}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ site_id: siteId }),
      },
      15000,
    );
    if (!response.ok || !body.status) {
      throw new Error(body.message || `Website ${action} failed`);
    }
    await refreshDashboard();
  } catch (error) {
    window.alert(error?.message || `Website ${action} failed`);
  } finally {
    delete websiteState.pendingActions[siteId];
    renderWebsiteTable();
  }
}

function renderWebsiteActionMenu() {
  const host = document.getElementById("website-action-menu-host");
  if (!host) return;

  const item = websiteState.items.find((entry) => entry.id === websiteState.openMenuId);
  if (!item || !websiteState.menuPosition) {
    host.hidden = true;
    host.innerHTML = "";
    return;
  }

  host.hidden = false;
  host.innerHTML = `
    <div class="website-action-menu website-action-menu-floating" role="menu">
      <button class="website-action-menu-item is-disabled" type="button" disabled>
        <span class="website-action-menu-icon" aria-hidden="true">${websiteActionMenuIcon("security")}</span>
        <span>Security Scan</span>
      </button>
      <button class="website-action-menu-item is-disabled" type="button" disabled>
        <span class="website-action-menu-icon" aria-hidden="true">${websiteActionMenuIcon("category")}</span>
        <span>Category</span>
      </button>
      <button
        class="website-action-menu-item is-danger"
        type="button"
        data-website-delete="${escapeHtml(item.id)}"
        ${websiteState.pendingDeleteId === item.id ? " disabled" : ""}
      >
        <span class="website-action-menu-icon" aria-hidden="true">${websiteActionMenuIcon("delete")}</span>
        <span>${websiteState.pendingDeleteId === item.id ? "Deleting..." : "Delete site"}</span>
      </button>
    </div>
  `;

  const menu = host.firstElementChild;
  if (!menu) return;

  requestAnimationFrame(() => {
    const margin = 12;
    const menuWidth = menu.offsetWidth || 168;
    const menuHeight = menu.offsetHeight || 132;
    let left = websiteState.menuPosition.left;
    let top = websiteState.menuPosition.top;

    left = Math.max(margin, Math.min(left, window.innerWidth - menuWidth - margin));
    if (top + menuHeight > window.innerHeight - margin) {
      top = Math.max(margin, websiteState.menuPosition.anchorTop - menuHeight - 8);
    }

    menu.style.left = `${left}px`;
    menu.style.top = `${top}px`;
  });
}

function buildWebsiteSparkline(requests, index) {
  const ratio = Math.min(1, Math.log10((requests || 0) + 10) / 6);
  const amplitude = 1.2 + ratio * 3.4;
  const baseline = 12.5;
  const width = 96;
  const values = Array.from({ length: 16 }, (_, pointIndex) => {
    const wave = Math.abs(Math.sin((pointIndex + 1) * 0.72 + index * 0.35)) * amplitude;
    const spike = pointIndex === (index * 3 + 4) % 16 ? amplitude * 0.55 : 0;
    return baseline - (wave + spike);
  });
  const step = width / (values.length - 1);
  const points = values.map((value, pointIndex) => `${(pointIndex * step).toFixed(2)},${value.toFixed(2)}`).join(" ");
  return `
    <svg viewBox="0 0 96 16" preserveAspectRatio="none" aria-hidden="true">
      <polyline points="${points}" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"></polyline>
    </svg>
  `;
}

function updateWebsiteBatchState(pageItems) {
  const checkAll = document.getElementById("website-check-all");
  const executeButton = document.getElementById("website-batch-execute");
  const batchTrigger = document.getElementById("website-batch-trigger");
  const batchLabel = document.getElementById("website-batch-label");
  const batchMenu = document.getElementById("website-batch-menu");
  if (!checkAll || !executeButton || !batchTrigger || !batchLabel || !batchMenu) return;
  const hasSelection = websiteState.selected.size > 0;
  const visibleIds = pageItems.map((item) => item.id);
  const allVisibleSelected = visibleIds.length > 0 && visibleIds.every((id) => websiteState.selected.has(id));
  checkAll.checked = allVisibleSelected;
  batchLabel.textContent = WEBSITE_BATCH_OPTIONS[websiteState.batchAction] || WEBSITE_BATCH_OPTIONS[""];
  batchTrigger.setAttribute("aria-expanded", websiteState.batchMenuOpen ? "true" : "false");
  batchTrigger.classList.toggle("is-open", websiteState.batchMenuOpen);
  batchMenu.hidden = !websiteState.batchMenuOpen;
  batchMenu.querySelectorAll("[data-website-batch-action]").forEach((option) => {
    option.classList.toggle("is-selected", option.dataset.websiteBatchAction === websiteState.batchAction);
  });
  executeButton.disabled = !hasSelection || !websiteState.batchAction || websiteState.batchPending;
  executeButton.textContent = websiteState.batchPending ? "Working..." : "Execute";
}

function closeWebsiteBatchMenu() {
  if (!websiteState.batchMenuOpen) return;
  websiteState.batchMenuOpen = false;
  const { pageItems } = getWebsiteView();
  updateWebsiteBatchState(pageItems);
}

async function executeWebsiteBatchAction() {
  if (websiteState.batchPending || !websiteState.batchAction || !websiteState.selected.size) return;

  const selectedIds = [...websiteState.selected];
  websiteState.batchPending = true;
  const { pageItems } = getWebsiteView();
  updateWebsiteBatchState(pageItems);

  try {
    if (websiteState.batchAction === "delete") {
      openWebsiteBatchDeleteModal(selectedIds);
      return;
    }

    if (websiteState.batchAction === "ssl") {
      for (const siteId of selectedIds) {
        const { response, body } = await fetchJsonWithTimeout(
          "/website/ssl",
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ site_id: siteId }),
          },
          20000,
        );
        if (!response.ok || !body.status) {
          throw new Error(body.message || "SSL setup failed");
        }
      }

      await refreshDashboard();
      return;
    }

    const executeButton = document.getElementById("website-batch-execute");
    const label = executeButton?.textContent || "Execute";
    if (executeButton) executeButton.textContent = "Queued";
    setTimeout(() => {
      if (executeButton && !websiteState.batchPending) {
        executeButton.textContent = label;
      }
    }, 900);
  } catch (error) {
    window.alert(error?.message || "Batch action failed");
  } finally {
    websiteState.batchPending = false;
    const nextPageItems = getWebsiteView().pageItems;
    updateWebsiteBatchState(nextPageItems);
  }
}

function syncWebsiteProjectTabs() {
  const availableProjects = new Set(websiteState.items.map((item) => item.category));
  if (availableProjects.size && !availableProjects.has(websiteState.project)) {
    websiteState.project = [...availableProjects][0];
  }

  document.querySelectorAll("[data-project-tab]").forEach((tab) => {
    const enabled = availableProjects.size === 0 || availableProjects.has(tab.dataset.projectTab);
    const active = tab.dataset.projectTab === websiteState.project;
    tab.disabled = !enabled;
    tab.classList.toggle("active", active);
    tab.setAttribute("aria-selected", active ? "true" : "false");
  });
}

function getWebsiteVisibleColumnCount() {
  const width = window.innerWidth || document.documentElement.clientWidth || 0;
  if (width <= 540) return 2;
  if (width <= 768) return 3;
  if (width <= 860) return 4;
  if (width <= 980) return 5;
  if (width <= 1180) return 6;
  return 10;
}

function renderWebsiteTable() {
  const tbody = document.getElementById("website-table-body");
  if (!tbody) return;
  const { filtered, totalPages, pageItems } = getWebsiteView();

  if (!pageItems.length) {
    tbody.innerHTML = `<tr class="website-empty"><td colspan="${getWebsiteVisibleColumnCount()}" style="text-align: center; padding: 40px; color: #94a3b8; font-style: italic;">domain empty</td></tr>`;
  } else {
    tbody.innerHTML = pageItems
      .map((item, index) => {
        const selected = websiteState.selected.has(item.id) ? "checked" : "";
        const statusClass = item.status === "running" ? "website-status-running" : "website-status-stopped";
        const backupClass = item.backup_total === 0 ? "is-warning" : "is-ok";
        const sslClass = websiteSslTone(item.ssl_status);
        const pendingAction = websiteState.pendingActions[item.id] || "";
        const nextAction = item.status === "running" ? "pause" : "start";
        const lifecycleLabel = pendingAction
          ? `${pendingAction === "pause" ? "Pausing" : "Starting"} site`
          : `${nextAction === "pause" ? "Pause" : "Start"} site`;
        return `
          <tr data-website-id="${escapeHtml(item.id)}">
            <td class="website-check-col" data-label="Select">
              <input class="website-row-check" type="checkbox" data-website-select="${escapeHtml(item.id)}" ${selected} aria-label="Select ${escapeHtml(item.name)}" />
            </td>
            <td class="website-site-col" data-label="Site name">
              <div class="website-site-cell">
                <span class="website-site-icon" aria-hidden="true">${websiteSiteIcon(item.ssl_enabled)}</span>
                <div class="website-site-meta">
                  <a href="${item.ssl_enabled ? 'https' : 'http'}://${escapeHtml(item.name)}" target="_blank" class="website-site-name ${item.ssl_enabled ? "is-https" : "is-http"}">
                    ${escapeHtml(item.name)}
                    <span class="website-site-external" aria-hidden="true">
                      <svg viewBox="0 0 20 20"><path d="M8 5h7v7"></path><path d="M15 5 7 13"></path><path d="M12 9v5H5V7h5"></path></svg>
                    </span>
                  </a>
                  <span class="website-site-alias">${escapeHtml(item.alias)}</span>
                </div>
              </div>
            </td>
            <td class="website-status-col" data-label="Status">
              <span class="website-status ${statusClass}">
                <button
                  class="website-status-toggle"
                  type="button"
                  data-website-lifecycle="${escapeHtml(item.id)}"
                  data-website-lifecycle-action="${nextAction}"
                  aria-label="${escapeHtml(lifecycleLabel)}"
                  title="${escapeHtml(lifecycleLabel)}"
                  ${pendingAction ? "disabled" : ""}
                >
                  <span class="website-status-icon" aria-hidden="true">${websiteStatusIcon(item.status)}</span>
                </button>
              </span>
            </td>
            <td class="website-backup-col" data-label="Backup">
              <span class="website-backup">
                <span class="website-backup-count">${escapeHtml(item.backup_total)}</span>
                <span class="website-backup-label ${backupClass}">${escapeHtml(item.backup_label)}</span>
              </span>
            </td>
            <td class="website-quick-col" data-label="Quick action">
              <span class="website-quick-actions">
                <span class="website-quick-icon" aria-hidden="true">${websiteQuickIcon("folder")}</span>
                <span class="website-quick-icon website-quick-icon-speed" aria-hidden="true">${websiteQuickIcon("speed")}</span>
                ${item.category === "PHP Project"
                  ? renderWebsitePhpSelect(item)
                  : `<span class="website-quick-runtime">${escapeHtml(item.runtime)}</span>`}
              </span>
            </td>
            <td class="website-expiration-col" data-label="Expiration">${escapeHtml(item.expiration)}</td>
            <td class="website-ssl-col" data-label="SSL"><span class="website-ssl ${sslClass}">${escapeHtml(item.ssl_status)}</span></td>
            <td class="website-requests-col" data-label="Requests">
              <div class="website-requests">
                <strong>${Number(item.requests || 0).toLocaleString()}</strong>
                <span class="website-requests-chart">${buildWebsiteSparkline(item.requests, index)}</span>
              </div>
            </td>
            <td class="website-waf-col" data-label="WAF"><span class="website-waf">${escapeHtml(item.waf)}</span></td>
            <td class="website-operate-col" data-label="Operate">
              <span class="website-operate-shell">
                <span class="website-operate">
                  <a class="website-operate-link" href="/website">Conf</a>
                  <a class="website-operate-link" href="/website">Log</a>
                  <button
                    class="website-operate-more"
                    type="button"
                    data-website-menu-toggle="${escapeHtml(item.id)}"
                    aria-label="Website actions"
                    aria-expanded="${websiteState.openMenuId === item.id ? "true" : "false"}"
                  >
                    <svg viewBox="0 0 20 20"><circle cx="10" cy="4.5" r="1"></circle><circle cx="10" cy="10" r="1"></circle><circle cx="10" cy="15.5" r="1"></circle></svg>
                  </button>
                </span>
              </span>
            </td>
          </tr>
        `;
      })
      .join("");
  }
  renderWebsiteActionMenu();
  renderWebsiteDeleteModal();

  document.getElementById("website-page-current").textContent = String(websiteState.page);
  document.getElementById("website-page-input").value = String(websiteState.page);
  document.getElementById("website-total-count").textContent = `Total ${filtered.length}`;
  document.getElementById("website-prev-page").disabled = websiteState.page <= 1;
  document.getElementById("website-next-page").disabled = websiteState.page >= totalPages;
  updateWebsiteBatchState(pageItems);
}

function getWebsiteDefaultRoot() {
  return websiteState.websiteRoot || "";
}

function sanitizeWebsiteDomainDraft(value = "") {
  return String(value || "").toLowerCase().replace(/\s+/g, "").trim();
}

function shouldSuggestDefaultWebsiteSuffix(value = "") {
  const draft = sanitizeWebsiteDomainDraft(value);
  return Boolean(draft) && !draft.includes(".") && /^[a-z0-9-]+$/.test(draft);
}

function finalizeWebsiteDomainValue(value = "") {
  const draft = sanitizeWebsiteDomainDraft(value);
  if (!draft) return "";
  return shouldSuggestDefaultWebsiteSuffix(draft) ? `${draft}${DEFAULT_WEBSITE_DOMAIN_SUFFIX}` : draft;
}

function normalizeWebsiteDomainLines(value = "") {
  return String(value || "")
    .split(/\r?\n/)
    .map((line) => finalizeWebsiteDomainValue(line))
    .filter(Boolean)
    .join("\n");
}

function getWebsiteDomainFolderName(domain) {
  return String(domain || "")
    .trim()
    .toLowerCase()
    .replace(/[:/\\]+/g, "-")
    .replace(/[^a-z0-9.-]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "")
    .replace(/\.+$/g, "");
}

function buildWebsiteDefaultPath(domain) {
  const root = getWebsiteDefaultRoot();
  const folder = getWebsiteDomainFolderName(finalizeWebsiteDomainValue(domain));
  if (!root) return folder;
  if (!folder) return root;
  return `${root}\\${folder}`;
}

function populateWebsiteCreatePhpOptions() {
  const select = document.getElementById("website-create-php-version");
  if (!select) return;
  const options = websiteState.phpRuntimes.map((runtime) => (
    `<option value="${escapeHtml(runtime.id)}">${escapeHtml(runtime.label)}</option>`
  ));
  options.push('<option value="">Static / No PHP</option>');
  select.innerHTML = options.join("");
}

function syncWebsiteDomainGhost() {
  const domainInput = document.getElementById("website-create-domain");
  const ghost = document.getElementById("website-create-domain-ghost");
  if (!domainInput || !ghost) return;

  const lines = domainInput.value.split("\n");
  const ghostHtml = lines
    .map((line) => {
      const trimmed = sanitizeWebsiteDomainDraft(line);
      if (trimmed && !trimmed.includes(".")) {
        return `<span style="visibility:hidden">${escapeHtml(trimmed)}</span><span class="ghost-suffix">${DEFAULT_WEBSITE_DOMAIN_SUFFIX}</span>`;
      }
      return escapeHtml(trimmed);
    })
    .join("\n");

  ghost.innerHTML = ghostHtml || "";
  ghost.scrollTop = domainInput.scrollTop;
}

function syncWebsiteCreatePathFromDomain(force = false) {
  const domainInput = document.getElementById("website-create-domain");
  const pathInput = document.getElementById("website-create-path");
  if (!domainInput || !pathInput) return;
  const firstDomain = domainInput.value
    .split(/\r?\n/)
    .map((value) => finalizeWebsiteDomainValue(value))
    .find(Boolean) || "";
  const suggested = buildWebsiteDefaultPath(firstDomain);
  if (force || !pathInput.value.trim() || pathInput.dataset.autoPath === "true") {
    pathInput.value = suggested;
    pathInput.dataset.autoPath = "true";
  }
}

function openWebsiteCreateModal() {
  const modal = document.getElementById("website-create-modal");
  const domainInput = document.getElementById("website-create-domain");
  const descriptionInput = document.getElementById("website-create-description");
  const pathInput = document.getElementById("website-create-path");
  const phpSelect = document.getElementById("website-create-php-version");
  const htmlToggle = document.getElementById("website-create-html");
  if (!modal || !domainInput || !descriptionInput || !pathInput || !phpSelect || !htmlToggle) return;
  populateWebsiteCreatePhpOptions();
  domainInput.value = "";
  descriptionInput.value = "";
  phpSelect.value = websiteState.phpRuntimes[0]?.id || "";
  htmlToggle.checked = true;
  pathInput.value = getWebsiteDefaultRoot();
  const ghost = document.getElementById("website-create-domain-ghost");
  pathInput.dataset.autoPath = "true";
  if (ghost) ghost.innerHTML = "";
  modal.hidden = false;
  domainInput.focus();
}

function closeWebsiteCreateModal() {
  const modal = document.getElementById("website-create-modal");
  if (modal) modal.hidden = true;
}

function bindWebsiteControls() {
  if (!document.getElementById("website-table-body")) return;
  document.querySelectorAll("[data-project-tab]").forEach((button) => {
    button.addEventListener("click", () => {
      websiteState.project = button.dataset.projectTab;
      websiteState.page = 1;
      document.querySelectorAll("[data-project-tab]").forEach((tab) => {
        const active = tab === button;
        tab.classList.toggle("active", active);
        tab.setAttribute("aria-selected", active ? "true" : "false");
      });
      renderWebsiteTable();
    });
  });

  document.getElementById("website-category-select").addEventListener("change", (event) => {
    websiteState.statusFilter = event.target.value;
    websiteState.page = 1;
    renderWebsiteTable();
  });

  document.getElementById("website-search-input").addEventListener("input", (event) => {
    websiteState.search = event.target.value.trim().toLowerCase();
    websiteState.page = 1;
    renderWebsiteTable();
  });

  document.getElementById("website-page-size").addEventListener("change", (event) => {
    websiteState.pageSize = Math.max(1, Number(event.target.value) || 10);
    websiteState.page = 1;
    renderWebsiteTable();
  });

  document.getElementById("website-prev-page").addEventListener("click", () => {
    websiteState.page = Math.max(1, websiteState.page - 1);
    renderWebsiteTable();
  });

  document.getElementById("website-next-page").addEventListener("click", () => {
    const { totalPages } = getWebsiteView();
    websiteState.page = Math.min(totalPages, websiteState.page + 1);
    renderWebsiteTable();
  });

  document.getElementById("website-page-input").addEventListener("change", (event) => {
    const { totalPages } = getWebsiteView();
    websiteState.page = Math.min(totalPages, Math.max(1, Number(event.target.value) || 1));
    renderWebsiteTable();
  });

  document.getElementById("website-check-all").addEventListener("change", (event) => {
    const { pageItems } = getWebsiteView();
    pageItems.forEach((item) => {
      if (event.target.checked) {
        websiteState.selected.add(item.id);
      } else {
        websiteState.selected.delete(item.id);
      }
    });
    renderWebsiteTable();
  });

  document.getElementById("website-table-body").addEventListener("change", (event) => {
    const websiteId = event.target.dataset.websiteSelect;
    if (websiteId) {
      if (event.target.checked) {
        websiteState.selected.add(websiteId);
      } else {
        websiteState.selected.delete(websiteId);
      }
      renderWebsiteTable();
      return;
    }
  });

  document.getElementById("website-table-body").addEventListener("click", (event) => {
    const lifecycleButton = event.target.closest("[data-website-lifecycle]");
    if (lifecycleButton) {
      event.preventDefault();
      const siteId = lifecycleButton.dataset.websiteLifecycle;
      const action = lifecycleButton.dataset.websiteLifecycleAction;
      runWebsiteLifecycleAction(siteId, action);
      return;
    }

    const menuToggle = event.target.closest("[data-website-menu-toggle]");
    if (menuToggle) {
      event.preventDefault();
      event.stopPropagation();
      const menuId = menuToggle.dataset.websiteMenuToggle;
      if (websiteState.openMenuId === menuId) {
        websiteState.openMenuId = null;
        websiteState.menuPosition = null;
      } else {
        const rect = menuToggle.getBoundingClientRect();
        websiteState.openMenuId = menuId;
        websiteState.menuPosition = {
          top: rect.bottom + 8,
          left: rect.right - 168,
          anchorTop: rect.top,
        };
      }
      renderWebsiteTable();
    }
  });

  const actionMenuHost = document.getElementById("website-action-menu-host");
  if (actionMenuHost) {
    actionMenuHost.addEventListener("click", (event) => {
      const deleteButton = event.target.closest("[data-website-delete]");
      if (deleteButton) {
        event.preventDefault();
        event.stopPropagation();
        openWebsiteDeleteModal(deleteButton.dataset.websiteDelete);
        return;
      }
      event.stopPropagation();
    });
  }

  document.addEventListener("click", () => {
    closeWebsiteBatchMenu();
    if (!websiteState.openMenuId) return;
    websiteState.openMenuId = null;
    websiteState.menuPosition = null;
    renderWebsiteTable();
  });

  window.addEventListener("resize", () => {
    closeWebsiteBatchMenu();
    if (!websiteState.openMenuId) return;
    websiteState.openMenuId = null;
    websiteState.menuPosition = null;
    renderWebsiteTable();
  });

  window.addEventListener("scroll", () => {
    closeWebsiteBatchMenu();
    if (!websiteState.openMenuId) return;
    websiteState.openMenuId = null;
    websiteState.menuPosition = null;
    renderWebsiteTable();
  }, true);

  document.addEventListener("keydown", (event) => {
    if (!websiteState.deleteDialog.open) return;
    if (event.key === "Escape") {
      closeWebsiteDeleteModal();
      return;
    }
    if (event.key === "Enter" && !websiteState.pendingDeleteId && isWebsiteDeleteVerificationValid()) {
      const activeElement = document.activeElement;
      if (activeElement && activeElement.id === "website-delete-verify-input") {
        event.preventDefault();
        deleteWebsiteSite();
      }
    }
  });

  document.getElementById("website-batch-trigger").addEventListener("click", (event) => {
    event.preventDefault();
    event.stopPropagation();
    websiteState.batchMenuOpen = !websiteState.batchMenuOpen;
    const { pageItems } = getWebsiteView();
    updateWebsiteBatchState(pageItems);
  });

  document.getElementById("website-batch-menu").addEventListener("click", (event) => {
    const option = event.target.closest("[data-website-batch-action]");
    if (!option) return;
    event.preventDefault();
    event.stopPropagation();
    websiteState.batchAction = option.dataset.websiteBatchAction || "";
    websiteState.batchMenuOpen = false;
    const { pageItems } = getWebsiteView();
    updateWebsiteBatchState(pageItems);
  });

  document.getElementById("website-batch-execute").addEventListener("click", (event) => {
    if (event.currentTarget.disabled) return;
    executeWebsiteBatchAction();
  });

  const addSiteButton = document.getElementById("website-add-site-button");
  const createModal = document.getElementById("website-create-modal");
  const createClose = document.getElementById("website-create-close");
  const createCancel = document.getElementById("website-create-cancel");
  const createForm = document.getElementById("website-create-form");
  const createDomain = document.getElementById("website-create-domain");
  const createPath = document.getElementById("website-create-path");
  const createPathReset = document.getElementById("website-create-path-reset");
  const deleteModal = document.getElementById("website-delete-modal");
  const deleteClose = document.getElementById("website-delete-close");
  const deleteCancel = document.getElementById("website-delete-cancel");
  const deleteConfirm = document.getElementById("website-delete-confirm");
  const deleteDocumentRoot = document.getElementById("website-delete-document-root");
  const deleteVerifyInput = document.getElementById("website-delete-verify-input");
  if (addSiteButton) addSiteButton.addEventListener("click", openWebsiteCreateModal);
  if (createClose) createClose.addEventListener("click", closeWebsiteCreateModal);
  if (createCancel) createCancel.addEventListener("click", closeWebsiteCreateModal);
  if (createModal) {
    createModal.addEventListener("click", (event) => {
      if (event.target.hasAttribute("data-website-create-close")) {
        closeWebsiteCreateModal();
      }
    });
  }
  if (deleteClose) deleteClose.addEventListener("click", closeWebsiteDeleteModal);
  if (deleteCancel) deleteCancel.addEventListener("click", closeWebsiteDeleteModal);
  if (deleteConfirm) deleteConfirm.addEventListener("click", () => {
    deleteWebsiteSite();
  });
  if (deleteDocumentRoot) {
    deleteDocumentRoot.addEventListener("change", (event) => {
      websiteState.deleteDialog.deleteDocumentRoot = Boolean(event.target.checked);
      renderWebsiteDeleteModal();
    });
  }
  if (deleteVerifyInput) {
    deleteVerifyInput.addEventListener("input", (event) => {
      websiteState.deleteDialog.verifyInput = String(event.target.value || "");
      renderWebsiteDeleteModal();
    });
  }
  if (deleteModal) {
    deleteModal.addEventListener("click", (event) => {
      if (event.target.hasAttribute("data-website-delete-close")) {
        closeWebsiteDeleteModal();
      }
    });
  }
  if (createDomain) {
    createDomain.addEventListener("input", () => {
      syncWebsiteDomainGhost();
      syncWebsiteCreatePathFromDomain();
    });
    createDomain.addEventListener("keydown", (event) => {
      if (event.key === " " || event.key === "Enter") {
        const input = createDomain;
        const text = input.value;
        const start = input.selectionStart;
        const before = text.substring(0, start);
        const lastWordMatch = before.match(/([a-zA-Z0-9-]{1,63})$/);
        
        if (lastWordMatch && !lastWordMatch[0].includes(".")) {
          const suffix = DEFAULT_WEBSITE_DOMAIN_SUFFIX;
          const after = text.substring(start);
          
          event.preventDefault();
          input.value = before + suffix + after;
          const newPos = start + suffix.length;
          input.setSelectionRange(newPos, newPos);
          
          syncWebsiteDomainGhost();
          syncWebsiteCreatePathFromDomain();
        }
      }
    });
    createDomain.addEventListener("blur", () => {
      const normalized = normalizeWebsiteDomainLines(createDomain.value);
      if (normalized) {
        createDomain.value = normalized;
      }
      syncWebsiteDomainGhost();
      syncWebsiteCreatePathFromDomain();
    });
    createDomain.addEventListener("scroll", () => {
      const ghost = document.getElementById("website-create-domain-ghost");
      if (ghost) ghost.scrollTop = createDomain.scrollTop;
    });
  }
  if (createPath) {
    createPath.addEventListener("input", () => {
      createPath.dataset.autoPath = "false";
    });
  }
  if (createPathReset) {
    createPathReset.addEventListener("click", () => syncWebsiteCreatePathFromDomain(true));
  }
  if (createForm) {
    createForm.addEventListener("submit", async (event) => {
      event.preventDefault();
      const confirmButton = document.getElementById("website-create-confirm");
      const domainInput = document.getElementById("website-create-domain");
      const normalizedDomains = normalizeWebsiteDomainLines(domainInput.value);
      domainInput.value = normalizedDomains;
      syncWebsiteDomainGhost();
      syncWebsiteCreatePathFromDomain();
      const sslCheckbox = document.getElementById("website-create-ssl");
      const payload = {
        domain: normalizedDomains,
        description: document.getElementById("website-create-description").value,
        website_path: document.getElementById("website-create-path").value,
        php_runtime_id: document.getElementById("website-create-php-version").value,
        create_html: document.getElementById("website-create-html").checked,
        apply_ssl: sslCheckbox ? sslCheckbox.checked : false,
      };
      if (confirmButton) {
        confirmButton.disabled = true;
        confirmButton.textContent = "Creating...";
      }
      try {
        const { response, body } = await fetchJsonWithTimeout(
          "/website/create",
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(payload),
          },
          20000,
        );
        if (!response.ok || !body.status) {
          throw new Error(body.message || `HTTP ${response.status}`);
        }
        closeWebsiteCreateModal();
        await refreshDashboard();
      } catch (error) {
        window.alert(error?.message || "Website creation failed");
      } finally {
        if (confirmButton) {
          confirmButton.disabled = false;
          confirmButton.textContent = "Confirm";
        }
      }
    });
  }
}

function updateOverview(data) {
  const sidebarHost = document.getElementById("sidebar-host");
  const topbarOsIcon = document.getElementById("topbar-os-icon");
  const topbarSystem = document.getElementById("topbar-system");
  const topbarUptime = document.getElementById("topbar-uptime");
  if (sidebarHost) sidebarHost.textContent = data.primary_ip;

  if (topbarOsIcon) {
    const os = (data.os_name || "").toLowerCase();
    let iconClass = "topbar-os-icon dashboard-icon iconfont ";
    if (os.includes("windows")) {
      iconClass += "icon-windows";
    } else if (os.includes("centos")) {
      iconClass += "icon-centos";
    } else if (os.includes("rocky")) {
      iconClass += "icon-rocky";
    } else if (os.includes("alma")) {
      iconClass += "icon-almalinux";
    } else if (os.includes("red hat") || os.includes("rhel")) {
      iconClass += "icon-redhat";
    } else if (os.includes("ubuntu")) {
      iconClass += "icon-ubuntu";
    } else if (os.includes("debian")) {
      iconClass += "icon-debian";
    } else {
      iconClass += "icon-linux";
    }
    topbarOsIcon.className = iconClass;
  }
  if (topbarSystem) topbarSystem.textContent = `System: ${data.os_name} (${data.kernel_version})`;
  if (topbarUptime) topbarUptime.textContent = formatAaPanelUptime(data.uptime);
  renderOverviewStats(data);
  softwareState.categories = Array.isArray(data.software_types) ? data.software_types : [];
  softwareState.items = Array.isArray(data.software_plugins) ? data.software_plugins : [];
  clearSoftwareOptimisticStateIfConfirmed(softwareState.items);
  websiteState.phpRuntimes = Array.isArray(data.php_runtimes) ? data.php_runtimes : [];
  if (softwareState.category !== "All" && softwareState.category !== "Installed") {
    const hasCategory = softwareState.categories.some((entry) => entry.title === softwareState.category)
      || softwareState.items.some((entry) => entry.category === softwareState.category);
    if (!hasCategory) {
      softwareState.category = "All";
    }
  }
  renderDashboardSoftwareSummary();
  renderSoftwareList();
  websiteState.items = Array.isArray(data.websites) ? data.websites : [];
  websiteState.websiteRoot = data.website_root || websiteState.websiteRoot || "";
  const validIds = new Set(websiteState.items.map((item) => item.id));
  websiteState.selected = new Set([...websiteState.selected].filter((id) => validIds.has(id)));
  if (websiteState.openMenuId && !validIds.has(websiteState.openMenuId)) {
    websiteState.openMenuId = null;
    websiteState.menuPosition = null;
  }
  if (websiteState.pendingDeleteId && websiteState.pendingDeleteId !== "__batch__" && !validIds.has(websiteState.pendingDeleteId)) {
    websiteState.pendingDeleteId = null;
  }
  if (websiteState.deleteDialog.mode === "batch") {
    websiteState.deleteDialog.siteIds = websiteState.deleteDialog.siteIds.filter((id) => validIds.has(id));
    if (!websiteState.deleteDialog.siteIds.length) {
      websiteState.deleteDialog = createWebsiteDeleteDialogState();
    }
  } else if (websiteState.deleteDialog.siteId && !validIds.has(websiteState.deleteDialog.siteId)) {
    websiteState.deleteDialog = createWebsiteDeleteDialogState();
  }
  syncWebsiteProjectTabs();
  populateWebsiteCreatePhpOptions();
  renderWebsiteTable();
}

function updateStatus(data) {
  if (!document.getElementById("load-meter")) return;
  const loadPercent = Math.min(100, (data.load_avg.one / Math.max(data.cpu_cores || 1, 1)) * 100);
  const memoryPercent = data.total_memory ? (data.used_memory / data.total_memory) * 100 : 0;
  const disk = data.app_disk;
  const diskUsed = disk ? Math.max(disk.total_space - disk.available_space, 0) : 0;
  const diskPercent = disk && disk.total_space ? (diskUsed / disk.total_space) * 100 : 0;
  const loadSummary = loadPercent < 60 ? "Smooth operation" : loadPercent < 85 ? "Moderate load" : "Busy";

  setMeter("load-meter", loadPercent);
  setMeter("cpu-meter", data.cpu_usage);
  setMeter("memory-meter", memoryPercent);
  setMeter("disk-meter", diskPercent);

  document.getElementById("load-meter-value").textContent = `${Math.round(loadPercent)}%`;
  document.getElementById("cpu-meter-value").textContent = `${Math.round(data.cpu_usage)}%`;
  document.getElementById("memory-meter-value").textContent = `${Math.round(memoryPercent)}%`;
  document.getElementById("disk-meter-value").textContent = `${Math.round(diskPercent)}%`;

  document.getElementById("load-label").textContent = data.load_avg.one.toFixed(2);
  document.getElementById("load-detail").textContent = `5m ${data.load_avg.five.toFixed(2)} - 15m ${data.load_avg.fifteen.toFixed(2)} - ${data.cpu_cores} cores`;
  document.getElementById("load-summary").textContent = loadSummary;

  document.getElementById("cpu-label").textContent = formatPercent(data.cpu_usage);
  document.getElementById("cpu-detail").textContent = `${data.cpu_brand} - ${data.cpu_frequency || "--"} MHz - ${data.process_count} processes`;
  document.getElementById("cpu-summary").textContent = `${data.cpu_cores} Core(s)`;

  document.getElementById("memory-label").textContent = formatPercent(memoryPercent);
  document.getElementById("memory-detail").textContent = `${formatBytes(data.used_memory)} / ${formatBytes(data.total_memory)} RAM - Swap ${formatBytes(data.used_swap)} / ${formatBytes(data.total_swap)}`;
  document.getElementById("memory-summary").textContent = `${formatBytes(data.used_memory)} / ${formatBytes(data.total_memory)}`;

  document.getElementById("disk-label").textContent = formatPercent(diskPercent);
  document.getElementById("disk-detail").textContent = disk ? `${formatBytes(diskUsed)} / ${formatBytes(disk.total_space)} - ${disk.mount_point}` : "Disk information unavailable";
  document.getElementById("disk-summary").textContent = disk ? `${formatBytes(diskUsed)} / ${formatBytes(disk.total_space)}` : "Disk unavailable";
}

function updateAlerts(data) {
  const list = document.getElementById("alert-list");
  const status = (data.alerts || [])[0] || "System status is healthy.";
  currentLogs = (data.alerts || []).map((message) => ({
    message,
    capturedAt: formatLogStamp(new Date()),
  }));
  currentLogSnapshot = formatLogStamp(new Date());
  const logButton = document.getElementById("sidebar-log-button");
  if (logButton) {
    logButton.textContent = String(data.warning_count || 0);
  }
  renderLogModal();
  if (!list) return;
  list.innerHTML = `
        <span class="footer-copy">MinPanel Linux panel ©2014-2026 MinPanel</span>
    <span class="footer-link">(${data.hostname})</span>
    <span class="footer-link">Forum</span>
    <span class="footer-link">Documentation</span>
    <span class="footer-support">Support:</span>
    <span class="support-chip"><span class="support-dot">T</span>Telegram</span>
    <span class="support-chip"><span class="support-dot">D</span>Discord</span>
  `;
}

function renderLogModal() {
  const list = document.getElementById("log-list");
  const meta = document.getElementById("log-modal-meta");
  if (!list || !meta) return;
  meta.textContent = `Last update: ${currentLogSnapshot}`;
  if (!currentLogs.length) {
    list.innerHTML = `<div class="log-empty">No warning logs right now.</div>`;
    return;
  }

  list.innerHTML = currentLogs
    .map((entry) => `
      <article class="log-item">
        <span class="log-item-time">${entry.capturedAt}</span>
        <p class="log-item-message">${entry.message}</p>
      </article>
    `)
    .join("");
}

function openLogModal() {
  const modal = document.getElementById("log-modal");
  if (modal) modal.hidden = false;
}

function closeLogModal() {
  const modal = document.getElementById("log-modal");
  if (modal) modal.hidden = true;
}

function updateTraffic(data) {
  if (!document.getElementById("upload-speed")) return;
  const networks = getNonLoopbackNetworks(data.networks);
  trafficState.networks = networks;
  populateNetworkSelect(networks);

  const sample = getSelectedTrafficSample(networks);
  document.getElementById("total-upload").textContent = formatBytes(sample.totalTransmitted);
  document.getElementById("total-download").textContent = formatBytes(sample.totalReceived);

  const now = new Date();
  const nowMs = now.getTime();
  let uploadRate = 0;
  let downloadRate = 0;
  const previous = trafficState.previousSamples[sample.key];

  if (previous) {
    const elapsedSeconds = Math.max((nowMs - previous.at) / 1000, 1);
    uploadRate = Math.max(sample.totalTransmitted - previous.totalTransmitted, 0) / elapsedSeconds;
    downloadRate = Math.max(sample.totalReceived - previous.totalReceived, 0) / elapsedSeconds;
  }

  trafficState.previousSamples[sample.key] = {
    totalTransmitted: sample.totalTransmitted,
    totalReceived: sample.totalReceived,
    at: nowMs,
  };

  trafficState.labels.push(now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" }));
  trafficState.upload.push(uploadRate);
  trafficState.download.push(downloadRate);

  if (trafficState.labels.length > 16) {
    trafficState.labels.shift();
    trafficState.upload.shift();
    trafficState.download.shift();
  }

  document.getElementById("upload-speed").textContent = formatTrafficSpeed(uploadRate);
  document.getElementById("download-speed").textContent = formatTrafficSpeed(downloadRate);
  drawTrafficChart();
}

function drawTrafficChart() {
  const canvas = document.getElementById("traffic-chart");
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  const width = canvas.width;
  const height = canvas.height;
  const padding = { top: 12, right: 18, bottom: 28, left: 48 };
  const fixedMaxValue = 120;
  const gridSteps = 6;
  ctx.clearRect(0, 0, width, height);

  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, width, height);

  const divisor = getTrafficUnitDivisor(trafficState.currentUnit);
  const uploadSeries = trafficState.upload.map((value) => value / divisor);
  const downloadSeries = trafficState.download.map((value) => value / divisor);
  const maxValue = fixedMaxValue;
  const plotWidth = width - padding.left - padding.right;
  const plotHeight = height - padding.top - padding.bottom;
  const points = Math.max(trafficState.labels.length - 1, 1);

  ctx.strokeStyle = "#d5e0ee";
  ctx.lineWidth = 1;
  for (let row = 0; row <= gridSteps; row += 1) {
    const y = padding.top + (plotHeight / gridSteps) * row;
    ctx.beginPath();
    ctx.moveTo(padding.left, y);
    ctx.lineTo(width - padding.right, y);
    ctx.stroke();
  }

  ctx.fillStyle = "#94a3b8";
  ctx.font = "12px Segoe UI";
  ctx.textAlign = "right";
  for (let row = 0; row <= gridSteps; row += 1) {
    const value = maxValue - (maxValue / gridSteps) * row;
    const y = padding.top + (plotHeight / gridSteps) * row + 4;
    ctx.fillText(String(Math.round(value)), padding.left - 8, y);
  }

  function getSeriesPoints(values) {
    return values.map((value, index) => ({
      x: padding.left + (plotWidth / points) * index,
      y: padding.top + plotHeight - (Math.min(value, maxValue) / maxValue) * plotHeight,
    }));
  }

  function traceSmoothPath(pointsData) {
    if (!pointsData.length) return;
    ctx.moveTo(pointsData[0].x, pointsData[0].y);
    if (pointsData.length === 1) return;

    for (let index = 0; index < pointsData.length - 1; index += 1) {
      const current = pointsData[index];
      const next = pointsData[index + 1];
      const controlX = (current.x + next.x) / 2;
      ctx.quadraticCurveTo(current.x, current.y, controlX, (current.y + next.y) / 2);
    }

    const last = pointsData[pointsData.length - 1];
    ctx.lineTo(last.x, last.y);
  }

  function drawFilledSeries(values, fill, crest) {
    if (!values.length) return;
    const pointsData = getSeriesPoints(values);

    ctx.beginPath();
    traceSmoothPath(pointsData);
    ctx.lineTo(pointsData[pointsData.length - 1].x, padding.top + plotHeight);
    ctx.lineTo(pointsData[0].x, padding.top + plotHeight);
    ctx.closePath();
    ctx.fillStyle = fill;
    ctx.fill();

    ctx.beginPath();
    traceSmoothPath(pointsData);
    ctx.strokeStyle = crest;
    ctx.lineWidth = 1.6;
    ctx.stroke();
  }

  function drawLineSeries(values, stroke) {
    if (!values.length) return;
    const pointsData = getSeriesPoints(values);
    ctx.beginPath();
    traceSmoothPath(pointsData);
    ctx.strokeStyle = stroke;
    ctx.lineWidth = 1.4;
    ctx.stroke();
  }

  drawFilledSeries(downloadSeries, "rgba(104, 171, 243, 0.72)", "#f8fbff");
  drawLineSeries(uploadSeries, "#f0ad2f");

  ctx.textAlign = "center";
  ctx.fillStyle = "#94a3b8";
  const desiredTicks = 9;
  const tickCount = Math.min(desiredTicks, trafficState.labels.length);
  for (let tickIndex = 0; tickIndex < tickCount; tickIndex += 1) {
    const sourceIndex = tickCount === 1
      ? trafficState.labels.length - 1
      : Math.round((tickIndex * (trafficState.labels.length - 1)) / (tickCount - 1));
    const x = padding.left + (plotWidth / points) * sourceIndex;
    ctx.fillText(trafficState.labels[sourceIndex], x, height - 10);
  }
}

async function refreshDashboard() {
  if (dashboardRefreshPromise) return dashboardRefreshPromise;

  dashboardRefreshPromise = (async () => {
    try {
      const route = normalizeDashboardPath(window.location.pathname).replace(/^\//, "") || "dashboard";
      const query = new URLSearchParams({ view: route });
      const { response, body: data } = await fetchJsonWithTimeout(
        `/dashboard/data?${query.toString()}`,
        { cache: "no-store" },
        10000,
      );
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      updateOverview(data);
      updateStatus(data);
      updateAlerts(data);
      updateTraffic(data);
    } catch (error) {
      const topbarSystem = document.getElementById("topbar-system");
      const topbarUptime = document.getElementById("topbar-uptime");
      const message = error?.name === "AbortError" ? "Request timeout" : error.message;
      if (topbarSystem) topbarSystem.textContent = `System: No connection (${message})`;
      if (topbarUptime) topbarUptime.textContent = "--";
    } finally {
      dashboardRefreshPromise = null;
    }
  })();

  return dashboardRefreshPromise;
}

document.addEventListener("DOMContentLoaded", () => {
  syncDashboardRoute();
  bindSoftwareControls();
  renderDashboardSoftwareSummary();
  renderSoftwareList();
  bindWebsiteControls();
  renderWebsiteTable();
  renderLogModal();

  document.querySelectorAll("[data-traffic-tab]").forEach((button) => {
    button.addEventListener("click", () => setTrafficTab(button.dataset.trafficTab));
  });
  const trafficNetworkSelect = document.getElementById("traffic-network-select");
  if (trafficNetworkSelect) {
    trafficNetworkSelect.addEventListener("change", (event) => {
      trafficState.currentSelection = event.target.value;
      trafficState.labels = [];
      trafficState.upload = [];
      trafficState.download = [];
      refreshDashboard();
    });
  }
  const trafficUnitSelect = document.getElementById("traffic-unit-select");
  if (trafficUnitSelect) {
    trafficUnitSelect.addEventListener("change", (event) => {
      trafficState.currentUnit = event.target.value;
      drawTrafficChart();
      const uploadSpeed = document.getElementById("upload-speed");
      const downloadSpeed = document.getElementById("download-speed");
      if (uploadSpeed) uploadSpeed.textContent = formatTrafficSpeed(trafficState.upload.at(-1) || 0);
      if (downloadSpeed) downloadSpeed.textContent = formatTrafficSpeed(trafficState.download.at(-1) || 0);
    });
    setTrafficTab("traffic");
  }

  refreshDashboard();
  setInterval(() => {
    if (window.location.pathname === "/software" && hasPendingSoftwareActions()) {
      return;
    }
    refreshDashboard();
  }, 4000);

  const sidebarLogButton = document.getElementById("sidebar-log-button");
  const logModalClose = document.getElementById("log-modal-close");
  const logBackdropClose = document.querySelector("[data-log-close]");
  if (sidebarLogButton) sidebarLogButton.addEventListener("click", openLogModal);
  if (logModalClose) logModalClose.addEventListener("click", closeLogModal);
  if (logBackdropClose) logBackdropClose.addEventListener("click", closeLogModal);

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeLogModal();
      closeWebsiteCreateModal();
      closeMessagesModal();
    }
  });

  initTaskManager();

  // Shadow the original runSoftwareAction with a version that opens the Messages Box on install
  const originalRunSoftwareAction = window.runSoftwareAction || runSoftwareAction;
  
  window.runSoftwareAction = async (id, action) => {
    // Only intercept the "install" action specifically
    if (action === "install") {
        try {
            const response = await fetch("/software/install", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ id }),
            });
            const result = await response.json().catch(() => ({ status: false }));
            if (result.status && result.message) {
                // message contains task_id for tracking
                openMessagesModal(result.message);
                renderSoftwareList();
                return;
            } else if (!result.status) {
                throw new Error(result.message || "Failed to trigger installation");
            }
        } catch (e) {
            console.error("Install trigger failed:", e);
            window.alert("Installation failed: " + e.message);
            return;
        }
    }
    
    // For all other actions (start, stop, uninstall), use the original logic
    return originalRunSoftwareAction(id, action);
  };
});

// --- Task Manager & Messages Box ---
const taskState = {
  tasks: [],
  activeTaskId: null,
  pollingInterval: null,
  activeTab: "task-list",
  logs: {}, // Cache for task logs
};

function initTaskManager() {
  const messagesModalClose = document.getElementById("messages-modal-close");
  const messagesBackdropClose = document.querySelector("[data-messages-close]");
  const messagesMenu = document.getElementById("messages-menu");

  if (messagesModalClose) messagesModalClose.onclick = closeMessagesModal;
  if (messagesBackdropClose) messagesBackdropClose.onclick = closeMessagesModal;

  if (messagesMenu) {
    messagesMenu.querySelectorAll("li").forEach((li) => {
      li.onclick = () => {
        const tab = li.getAttribute("data-tab");
        switchMessagesTab(tab);
      };
    });
  }

  // Start background task list polling
  setInterval(refreshTasks, 5000);
}

function openMessagesModal(taskId = null) {
  const modal = document.getElementById("messages-modal");
  if (modal) modal.hidden = false;
  
  if (taskId) {
    taskState.activeTaskId = taskId;
    switchMessagesTab("task-list");
    startTaskLogPolling(taskId);
  } else {
    refreshTasks();
  }
}

function closeMessagesModal() {
  const modal = document.getElementById("messages-modal");
  if (modal) modal.hidden = true;
  stopTaskLogPolling();
}

function switchMessagesTab(tabId) {
  taskState.activeTab = tabId;
  
  // Stop existing polling to reset state
  stopTaskLogPolling();

  // If switching to task-list without a running task, we don't need a specific activeTaskId for the full log
  const hasRunning = taskState.tasks.some(t => t.status === "running");
  if (tabId === "task-list" && !hasRunning) {
    // Keep activeTaskId if it was just set by openMessagesModal, but otherwise consider it "general view"
  }

  // Update menu UI
  const menu = document.getElementById("messages-menu");
  if (menu) {
    menu.querySelectorAll("li").forEach((li) => {
      li.classList.toggle("active", li.getAttribute("data-tab") === tabId);
    });
  }

  // Update content UI
  const tabs = ["task-list", "message-list", "execution-log"];
  tabs.forEach((t) => {
    const el = document.getElementById(`tab-${t}`);
    if (el) el.hidden = t !== tabId;
  });

  if (tabId === "task-list") refreshTasks();
  
  // Only restart polling if we are on the log tab and have a task,
  // OR if we are on the task list and a task is running (for the mini-log)
  if (taskState.activeTaskId) {
    if (tabId === "execution-log" || (tabId === "task-list" && hasRunning)) {
      startTaskLogPolling(taskState.activeTaskId);
    }
  }
}

async function refreshTasks() {
  try {
    const response = await fetch("/tasks");
    if (!response.ok) return;
    let tasks = await response.json();
    
    // Focused view: only show the running task, or the latest one
    const running = tasks.filter(t => t.status === "running");
    if (running.length > 0) {
       tasks = [running[0]];
       // Automatically show console for the running task in Task List
       if (!taskState.activeTaskId || taskState.activeTaskId !== tasks[0].id) {
           taskState.activeTaskId = tasks[0].id;
           if (taskState.activeTab === "task-list" || taskState.activeTab === "execution-log") {
             startTaskLogPolling(tasks[0].id);
           }
       }
    } else if (tasks.length > 0) {
       tasks = [tasks[0]];
    }

    taskState.tasks = tasks;
    renderTaskList();
    updateTaskBadge(tasks.length);
  } catch (err) {
    console.error("Failed to refresh tasks:", err);
  }
}

function updateTaskBadge(count) {
  const badge = document.getElementById("task-count-badge");
  if (badge) badge.textContent = `(${count})`;
}

function scrollToBottom(el) {
  if (!el) return;
  requestAnimationFrame(() => {
    el.scrollTop = el.scrollHeight;
  });
}

function renderTaskList() {
  const host = document.getElementById("messages-active-tasks");
  if (!host) return;

  if (taskState.tasks.length === 0) {
    host.innerHTML = '<div class="messages-empty-state">No active tasks.</div>';
    return;
  }

    host.innerHTML = taskState.tasks
    .map((task) => `
      <div class="task-item-group">
        <div class="task-item-row">
          <div class="task-item-info">
            <span class="task-dot is-${task.status}"></span>
            <span class="task-name">${escapeHtml(task.name)}</span>
          </div>
          <div class="task-status-actions">
            ${(() => {
              if (task.status === "running") {
                const match = task.last_message?.match(/(\d+)%/);
                if (match) {
                  const percent = match[1];
                  return `
                    <div class="task-progress-container">
                      <div class="task-progress-track">
                        <div class="task-progress-bar" style="width: ${percent}%"></div>
                      </div>
                      <span class="task-progress-text">${percent}%</span>
                    </div>
                  `;
                }
                return `<span class="task-status-text">${escapeHtml(task.last_message)}</span>`;
              } else if (task.status === "failed") {
                 return `<span class="task-status-text is-error">${escapeHtml(task.last_message || "Failed")}</span>`;
              }
              return `<span class="task-status-text">${getTaskStatusText(task.status)}</span>`;
            })()}
            <span class="task-divider">|</span>
            <a class="task-delete-link" onclick="viewTaskLog('${task.id}')">View Log</a>
          </div>
        </div>
        ${task.id === taskState.activeTaskId ? `<div id="task-log-${task.id}" class="messages-log-container">${ansiToHtml(taskState.logs[task.id] || "Loading log...")}</div>` : ""}
      </div>
    `)
    .join("");

  if (taskState.activeTaskId) {
    const el = document.getElementById(`task-log-${taskState.activeTaskId}`);
    if (el) scrollToBottom(el);
  }
}

function getTaskStatusColor(status) {
  switch (status) {
    case "running": return "#22c55e";
    case "success": return "#22c55e";
    case "failed": return "#ef4444";
    default: return "#94a3b8";
  }
}

function getTaskStatusText(status) {
  switch (status) {
    case "running": return "Installing ....";
    case "success": return "Success";
    case "failed": return "Failed";
    default: return "Pending";
  }
}

function viewTaskLog(taskId) {
  taskState.activeTaskId = taskId;
  switchMessagesTab("execution-log");
  startTaskLogPolling(taskId);
}

function startTaskLogPolling(taskId) {
  stopTaskLogPolling();
  
  const poll = async () => {
    try {
      const response = await fetch(`/tasks/${taskId}/log`);
      if (!response.ok) return;
      const data = await response.json();
      
      // Cache the log
      taskState.logs[taskId] = data.log;
      
      // Update containers only if they are relevant to the current view
      const miniLog = document.getElementById(`task-log-${taskId}`);
      if (miniLog) {
          miniLog.innerHTML = ansiToHtml(data.log);
          scrollToBottom(miniLog);
      }
      
      if (taskState.activeTab === "execution-log") {
          const mainLog = document.getElementById("tab-execution-log");
          if (mainLog) {
            mainLog.innerHTML = `<div class="messages-log-container">${ansiToHtml(data.log)}</div>`;
            scrollToBottom(mainLog.firstElementChild);
          }
      }

      if (data.status !== "running") {
        stopTaskLogPolling();
        refreshTasks();
      }
    } catch (err) {
      console.error("Log polling failed:", err);
    }
  };

  poll();
  taskState.pollingInterval = setInterval(poll, 1500);
}

function stopTaskLogPolling() {
  if (taskState.pollingInterval) {
    clearInterval(taskState.pollingInterval);
    taskState.pollingInterval = null;
  }
}

function ansiToHtml(text) {
  // Simple "ANSI" converter for basic shell simulation
  return escapeHtml(text)
    .replace(/\n/g, "<br>")
    .replace(/\[\d+m/g, ""); // Remove basic color codes
}

function escapeHtml(text) {
  const div = document.createElement("div");
  div.textContent = text;
  return div.innerHTML;
}

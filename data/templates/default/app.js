(() => {
  const app = window.MinPanel;
  if (!app?.runPageInitializers) return;

  function initTrafficControls() {
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
  }

  function initLogModalControls() {
    const sidebarLogButton = document.getElementById("sidebar-log-button");
    const logModalClose = document.getElementById("log-modal-close");
    const logBackdropClose = document.querySelector("[data-log-close]");

    if (sidebarLogButton) sidebarLogButton.addEventListener("click", openLogModal);
    if (logModalClose) logModalClose.addEventListener("click", closeLogModal);
    if (logBackdropClose) logBackdropClose.addEventListener("click", closeLogModal);
  }

  function initEscapeShortcuts() {
    document.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      closeSoftwareInstallModal();
      closeSoftwareSettingsModal();
      closeLogModal();
      closeWebsiteCreateModal();
      closeDatabaseCreateModal();
      closePhpMyAdminModal();
      closeMessagesModal();
    });
  }

  function initAutoRefresh() {
    refreshDashboard();
    window.setInterval(() => {
      if (window.location.pathname === "/software" && hasPendingSoftwareActions()) {
        return;
      }
      refreshDashboard();
    }, 4000);
  }

  function interceptSoftwareInstallActions() {
    const originalRunSoftwareAction = window.runSoftwareAction || runSoftwareAction;

    window.runSoftwareAction = async (id, action) => {
      if (action === "install") {
        try {
          const response = await fetch("/software/install", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ id }),
          });
          const result = await response.json().catch(() => ({ status: false }));
          if (result.status && result.message) {
            openMessagesModal(result.message);
            renderSoftwareList();
            return;
          }
          if (!result.status) {
            throw new Error(result.message || "Failed to trigger installation");
          }
        } catch (error) {
          console.error("Install trigger failed:", error);
          window.alert("Installation failed: " + error.message);
          return;
        }
      }

      return originalRunSoftwareAction(id, action);
    };
  }

  function bootstrapDefaultTheme() {
    if (app.defaultThemeBootstrapped) return;
    app.defaultThemeBootstrapped = true;

    syncDashboardRoute();
    app.runPageInitializers();
    renderLogModal();
    initTrafficControls();
    initLogModalControls();
    initEscapeShortcuts();
    initTaskManager();
    interceptSoftwareInstallActions();
    initAutoRefresh();
  }

  app.bootstrapDefaultTheme = bootstrapDefaultTheme;

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", bootstrapDefaultTheme, { once: true });
  } else {
    bootstrapDefaultTheme();
  }
})();

(() => {
  const app = window.MinPanel;
  if (!app?.addPageInitializer) return;

  app.addPageInitializer("software", () => {
    bindSoftwareControls();
    renderSoftwareList();
  });
})();

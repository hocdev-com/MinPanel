(() => {
  const app = window.MinPanel;
  if (!app?.addPageInitializer) return;

  app.addPageInitializer("database", () => {
    bindDatabaseControls();
    renderDatabaseTable();
  });
})();

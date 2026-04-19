# Repository Instructions

## Workspace Intent

- `aaPanel/` is a local reference snapshot for product behavior, copy, layout, and implementation ideas.
- `src/` is the active mini-panel codebase that we edit.

## Rules For Codex And Other Agents

- Do not modify files under `aaPanel/` unless the user explicitly asks to edit that directory.
- Treat `aaPanel/` as read-only reference material by default.
- When implementing or adjusting a feature in `src/`, check whether the same feature or UI pattern exists in `aaPanel/` first.
- Prefer matching aaPanel behavior and wording when the user asks for aaPanel-style UI or feature parity.
- Keep all new implementation work in the mini-panel app unless the user clearly requests changes to the reference snapshot.

## Practical Workflow

- Use `aaPanel/` to inspect templates, scripts, and naming before changing `src/`.
- Copy behavior, not files: adapt the reference into the Rust mini-panel architecture instead of patching aaPanel directly.
- If aaPanel and mini-panel differ, preserve mini-panel's runtime model while aligning UI and feature behavior as closely as practical.

## aaPanel Parity Checklist

- Locate the equivalent feature, template, or script inside `aaPanel/` before editing `src/`.
- Confirm whether the user wants visual similarity, behavior parity, or both.
- Reuse aaPanel wording, labels, and information hierarchy when practical.
- Implement the change in `src/`, not in `aaPanel/`.
- Keep a note of any intentional differences caused by Rust architecture, local platform limits, or missing backend data.

## Responsive UI Standards (Active Skill)

> [!IMPORTANT]
> **Automatic Documentation Rule**: Whenever a new significant UI pattern, technical "skill," or project-specific standard is established, the agent MUST automatically update this section in `AGENTS.md` to persist the knowledge for future sessions.

When modifying the dashboard layout, follow these synchronized responsive patterns to maintain consistency:

### 1. Unified Breakpoint (The 1100px Rule)
- All top-level dashboard sections (System Info, Sys Status, Overview) must trigger their responsive layouts together at **1100px**.
- This avoids intermediate "broken" states where some cards wrap while others stay horizontal.

### 2. Main Layout & Spacing
- **Sidebar-to-Content Gap**: Maintain a constant `24px` padding on the `.main` container for desktop views. This ensures the "white cards" never touch the "dark sidebar."
- **Mobile Container**: Adjust `.main` padding to `16px` or `20px` at lower breakpoints (e.g., 1100px) to balance screen real estate while preserving the gap.

### 3. System Info Card (Topbar)
- **Positioning**: Use `position: relative` on the container (`.topbar`).
- **Floating Actions**: On mobile/tablet, use `position: absolute; top: 18px; right: 18px;` for `.top-actions`.
- **Vertical Grouping**: Stack badges (`.top-plan`, `.top-meta`) above operation links (`.top-action-link`) in two distinct rows.
- **Overlap Prevention**: Apply `padding-right: 120px` to the main text container (`.topbar-main`) to ensure labels don't hide behind floating badges.

### 3. Metric Grids (Sys Status & Overview)
- **Persistent 2x2 Layout**: Grids must switch to `grid-template-columns: repeat(2, 1fr)` at 1100px and **maintain** this 2x2 structure even on the narrowest mobile screens.
- **Mobile Spacing**: Use `12px` to `16px` gaps/padding for mobile viewports to keep the layout compact but touch-friendly.
- **Meter Stability**: Ensure circular meters (`.meter`) scale appropriately (approx. 90px-110px) to fit side-by-side in the 2-column grid.

### 4. Website Page Responsive Pattern
- **Shared Breakpoint**: Website controls should join the same `1100px` breakpoint family used by the dashboard so toolbar/footer reflow happens before the table feels cramped.
- **Toolbar Reflow**: At tablet/mobile sizes, let `.website-toolbar-left` and `.website-toolbar-right` expand to full width; make search/select controls stretch instead of keeping fixed pixel widths.
- **Three-Control Toolbar Row**: On narrow Website layouts, keep category select, search field, and settings button on the same row using a 3-column grid instead of dropping search beneath the filter.
- **Narrowest Website Table**: On the smallest website breakpoint, keep a real table header instead of switching to card rows. Show only `Site name` and `Operate` so even an empty list still exposes the surviving columns.
- **Column Shedding Order**: On medium desktop widths, drop `WAF` first, then `Requests`, to protect `Site name` width. After that, when width gets tighter, drop `Quick action`, `SSL`, `Expiration`, then `Backup`, and finally `Status` only at the smallest table breakpoint. Preserve `Site name` as the main flexible column and keep `Operate` visible at the end.
- **Modal Stacking**: Website create/delete dialogs should switch label-value rows to a single-column stack on narrow screens and make footer actions full width.

### 5. Software Page Responsive Pattern
- **Smallest Software Table**: On the smallest software breakpoint, keep the table header visible but reduce the table to only `App Name` and `Operate`.
- **Software Empty State**: The software empty row must use a dynamic `colspan` that matches the currently visible columns, so the message stays centered after responsive column shedding.
- **Software Name Priority**: Let `App Name` take the flexible width on mobile and keep `Operate` as a tight trailing column.
- **No Horizontal Scroll**: Do not rely on horizontal scrolling for the software table on tablet/mobile widths. Remove the large fixed table minimum and shed columns progressively instead.
- **Software Column Shedding Order**: Hide `Instructions` and `Expire Time` first, then `Developer` and `Location`, then `Price`, and only at the smallest breakpoint hide `Status` so the table ends at `App Name + Operate`.
- **Software Display Toggle**: Keep the `Display` toggle column visible together with `Status` on desktop/tablet widths so operators can control the dashboard summary directly from the table; hide both only at the smallest breakpoint where the table collapses to `App Name + Operate`.
- **Software Secondary Actions**: Place `Update App List` inside the `Recently visited plugin` card, aligned to the right of that info strip instead of inside the filter rows.
- **Recently Strip Layout**: Keep the `Recently plugin` content and its right-side action on the same row when shrinking; let the text side compress first instead of dropping the action to a new line.
- **Software Settings Modal**: Keep `Setting` separate from the install/update version picker. It should open a dedicated two-pane modal with aaPanel-style left navigation and a service panel (`Stop`, `Restart`, `Reload`, alert toggle, daemon toggle); collapse the sidebar into wrapped chips on narrow screens instead of merging it back into the install modal.

### 6. Shared Close Button Pattern
- **Universal `X` Button**: Any dashboard/modal/dialog close button must use the shared circular close-button pattern, not a plain text `×`.
- **Base Look**: Render it as a gray circular button with a white `X` built from CSS lines, matching aaPanel-style floating closes.
- **Hover Motion**: On hover or keyboard focus, rotate the button `180deg` and switch the background to red.
- **Reuse First**: Prefer a shared utility class for new close buttons so future modals inherit the same size, transition, and icon treatment automatically.
- **Placement**: Use the floating top-right variant for overlay dialogs when practical; if a dialog keeps the close button inline in the header, keep the same visual treatment and hover behavior.

### 7. Runtime Lua Plugin Lifecycle
- When a runtime uses `data/plugins/*.lua`, the Rust install flow must call the Lua `on_install` hook after extraction and before `on_start`.
- The Lua bridge helper `panel.write_file(path, content)` must create the parent directory automatically so PID/config writes do not fail on fresh installs.
- Treat `on_install` as the place to prepare writable folders and generated config files such as `logs/`, `tmp/`, `php.ini`, or `my.ini`.
- Apache-specific routing/config generation belongs in `data/plugins/apache.lua`; Rust should pass structured website/runtime data into Lua instead of hardcoding `httpd.conf`, `vhost`, or Apache path rewrites in `src/dashboard.rs`.
- Path roots must support env overrides. Use `MINPANEL_WEBSITE_ROOT` for site files and `MINPANEL_RUNTIME_ROOT` for installed runtimes; if the env value is relative, resolve it against the app base directory first, then current working directory.
- Lua runtime hooks must raise real failures with `error(...)` instead of returning `"Error: ..."` strings, so Rust can treat startup/setup failures as errors immediately.
- `panel.spawn(...)` is for detached long-running runtimes and must reject processes that exit immediately after launch; use Apache/PHP error logs only as extra detail, not as the primary success signal.
- Use `panel.spawn_detached(...)` for self-daemonizing Windows runtimes that should return immediately to Rust without the early-exit guard from `panel.spawn(...)`.
- Apache on Windows must not use `panel.spawn(...)` for `httpd.exe`, because the parent process exits immediately after handing off worker children and will be misdetected as a startup failure. Do not use `httpd.exe -k start` either, because that expects a Windows service such as `Apache2.4`. Do not use `panel.execute(...)` either, because it keeps the hook blocked while `httpd.exe` runs in the foreground. Launch Apache with `panel.spawn_detached(httpd.exe, {"-d", ..., "-f", ...})`, then let Rust verify the running process/port after the hook returns.
- Apache should only enable `Listen 443` and SSL modules when at least one managed site actually has SSL configured, to avoid startup failures from unrelated port `443` conflicts.
- The default managed website root is `www` under the app base directory unless `MINPANEL_WEBSITE_ROOT` overrides it. Create that directory automatically during runtime resolution so Apache `DocumentRoot` never points to a missing folder.
- Windows listener parsing belongs in Rust with unit tests in `src/dashboard.rs`; do not rely on one-off verification scripts under `scratch/` for runtime port detection behavior.

use std::ffi::OsStr;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use windows_sys::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, InvalidateRect,
    LineTo, MoveToEx, ReleaseDC, SelectObject, SetBkMode, SetTextColor, UpdateWindow,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_LEFT,
    DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, HBRUSH, HDC, HGDIOBJ, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    PS_SOLID, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetClientRect, GetCursorPos, GetMessageW, GetSystemMetrics, LoadCursorW,
    LoadIconW, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW, SendMessageW, SetCursor,
    SetForegroundWindow, SetMenuItemBitmaps, ShowWindow, TrackPopupMenu, TranslateMessage,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, ICON_BIG, ICON_SMALL, IDC_ARROW, IDC_HAND,
    IDI_APPLICATION, MB_ICONERROR, MB_OK, MF_BYCOMMAND, MF_STRING, MSG, SW_HIDE, SW_RESTORE,
    SW_SHOW, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_RBUTTONUP, WM_SETCURSOR, WM_SETICON, WNDCLASSW, WS_CAPTION, WS_MINIMIZEBOX, WS_SYSMENU,
    WS_VISIBLE,
};

const WINDOW_CLASS_NAME: &str = "MinPanelControlShell";
const WINDOW_TITLE: &str = "MinPanel";
const TRAY_ICON_ID: u32 = 1001;
const MENU_SHOW_ID: usize = 2001;
const MENU_OPEN_ID: usize = 2002;
const MENU_EXIT_ID: usize = 2003;

const WM_APP_SYNC: u32 = WM_APP + 1;
const WM_APP_ERR: u32 = WM_APP + 2;
const WM_APP_RESTORE: u32 = WM_APP + 3;
const WM_TRAY_ICON: u32 = WM_APP + 4;

const WINDOW_WIDTH: i32 = 620;
const WINDOW_HEIGHT: i32 = 360;

static ACTIVE_SHELL: OnceLock<Mutex<Option<Arc<ControlPanelShell>>>> = OnceLock::new();

pub fn launch(preferred_port: u16) -> Result<(), String> {
    let controller = Arc::new(ServerController::new(preferred_port));
    controller.start()?;

    let shell = Arc::new(ControlPanelShell::new(controller.clone()));
    set_active_shell(shell.clone());

    let hwnd = create_main_window()?;
    shell.set_hwnd(hwnd);

    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
    }

    let mut message = unsafe { zeroed::<MSG>() };
    loop {
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }

        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    clear_active_shell();
    controller.stop()?;
    Ok(())
}

pub fn show_startup_error(title: &str, message: &str) {
    let title_w = to_wide(title);
    let message_w = to_wide(message);
    unsafe {
        MessageBoxW(
            null_mut(),
            message_w.as_ptr(),
            title_w.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

struct ServerController {
    preferred_port: u16,
    state: Mutex<ServerState>,
}

struct ServerState {
    running: bool,
    current_port: u16,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<JoinHandle<Result<(), String>>>,
}

impl ServerController {
    fn new(preferred_port: u16) -> Self {
        Self {
            preferred_port,
            state: Mutex::new(ServerState {
                running: false,
                current_port: preferred_port,
                shutdown: None,
                handle: None,
            }),
        }
    }

    fn start(&self) -> Result<(), String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "Failed to acquire server controller lock.".to_string())?;
        if state.running {
            return Ok(());
        }

        let preferred_port = self.preferred_port;
        let (startup_tx, startup_rx) = mpsc::sync_channel(1);
        let startup_tx_err = startup_tx.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let handle = thread::spawn(move || -> Result<(), String> {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("Failed to build Tokio runtime: {error}"))?;

            let result: Result<(), String> = runtime.block_on(async move {
                let listener = crate::bind_listener(preferred_port).await?;
                let port = listener
                    .local_addr()
                    .map_err(|error| format!("Failed to read bound listener address: {error}"))?
                    .port();
                let _ = startup_tx.send(Ok(port));

                axum::serve(listener, crate::app_router())
                    .with_graceful_shutdown(async {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .map_err(|error| format!("MinPanel server exited with error: {error}"))?;

                crate::stop_runtimes_on_shutdown().await?;
                Ok(())
            });

            if let Err(error) = &result {
                let _ = startup_tx_err.send(Err(error.clone()));
            }

            result
        });

        drop(state);

        let startup_status = startup_rx
            .recv()
            .map_err(|_| "MinPanel server failed to report startup state.".to_string())?;

        match startup_status {
            Ok(current_port) => {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| "Failed to update server controller state.".to_string())?;
                state.running = true;
                state.current_port = current_port;
                state.shutdown = Some(shutdown_tx);
                state.handle = Some(handle);
                Ok(())
            }
            Err(error) => {
                let _ = handle.join();
                Err(error)
            }
        }
    }

    fn stop(&self) -> Result<(), String> {
        let (shutdown, handle) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Failed to acquire server controller lock.".to_string())?;
            if !state.running {
                return Ok(());
            }

            state.running = false;
            (state.shutdown.take(), state.handle.take())
        };

        if let Some(shutdown) = shutdown {
            let _ = shutdown.send(());
        }

        if let Some(handle) = handle {
            match handle.join() {
                Ok(result) => result,
                Err(_) => Err("MinPanel server thread panicked.".to_string()),
            }
        } else {
            Ok(())
        }
    }

    fn is_running(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.running)
            .unwrap_or(false)
    }

    fn dashboard_url(&self) -> String {
        let port = self
            .state
            .lock()
            .map(|state| state.current_port)
            .unwrap_or(self.preferred_port);
        format!("http://localhost:{port}")
    }
}

struct ControlPanelShell {
    controller: Arc<ServerController>,
    hwnd: AtomicIsize,
    busy: AtomicBool,
    app_icon: isize,
    menu_icons: TrayMenuIcons,
    fonts: FontSet,
    state: Mutex<ShellState>,
}

struct TrayMenuIcons {
    show: isize,
    open: isize,
    exit: isize,
}

struct FontSet {
    title: isize,
    body: isize,
    small: isize,
    mono: isize,
    button: isize,
    caption: isize,
}

struct ShellState {
    tray_visible: bool,
    hover: HoverTarget,
    tracking_mouse: bool,
    last_error: Option<String>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum HoverTarget {
    None,
    Link,
    StartStop,
    Hide,
    Exit,
}

impl ControlPanelShell {
    fn new(controller: Arc<ServerController>) -> Self {
        let icon = load_embedded_app_icon();
        Self {
            controller,
            hwnd: AtomicIsize::new(0),
            busy: AtomicBool::new(false),
            app_icon: icon,
            menu_icons: TrayMenuIcons {
                show: create_tray_menu_icon(TrayMenuIconKind::Show, rgb(37, 99, 235)),
                open: create_tray_menu_icon(TrayMenuIconKind::Open, rgb(15, 118, 110)),
                exit: create_tray_menu_icon(TrayMenuIconKind::Exit, rgb(185, 28, 28)),
            },
            fonts: FontSet {
                title: create_font(-28, 700, "Segoe UI Semibold"),
                body: create_font(-18, 400, "Segoe UI"),
                small: create_font(-15, 400, "Segoe UI"),
                mono: create_font(-18, 500, "Consolas"),
                button: create_font(-18, 700, "Segoe UI Semibold"),
                caption: create_font(-14, 600, "Segoe UI"),
            },
            state: Mutex::new(ShellState {
                tray_visible: false,
                hover: HoverTarget::None,
                tracking_mouse: false,
                last_error: None,
            }),
        }
    }

    fn set_hwnd(&self, hwnd: HWND) {
        self.hwnd.store(hwnd as isize, Ordering::SeqCst);
    }

    fn hwnd(&self) -> HWND {
        self.hwnd.load(Ordering::SeqCst) as HWND
    }

    fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    fn hover(&self) -> HoverTarget {
        self.state
            .lock()
            .map(|state| state.hover)
            .unwrap_or(HoverTarget::None)
    }

    fn paint(&self) {
        let hwnd = self.hwnd();
        if hwnd.is_null() {
            return;
        }

        let mut paint = unsafe { zeroed::<PAINTSTRUCT>() };
        let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
        if hdc.is_null() {
            return;
        }

        let client = self.client_rect();
        fill_rect(hdc, client, rgb(8, 15, 31));
        fill_rect(hdc, rect(0, 0, client.right, 88), rgb(13, 28, 58));
        fill_rect(hdc, rect(0, 88, client.right, 96), rgb(20, 108, 214));

        fill_rect(hdc, rect(28, 118, client.right - 28, 196), rgb(16, 30, 56));
        fill_rect(hdc, rect(28, 212, 236, 292), rgb(16, 30, 56));
        fill_rect(hdc, self.quick_actions_rect(), rgb(16, 30, 56));

        fill_rect(hdc, rect(36, 20, 94, 78), rgb(36, 116, 255));
        draw_text(
            hdc,
            "AP",
            rect(36, 20, 94, 78),
            self.fonts.title,
            rgb(248, 251, 255),
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        draw_text(
            hdc,
            "MinPanel",
            rect(116, 20, client.right - 24, 52),
            self.fonts.title,
            rgb(248, 251, 255),
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
        draw_text(
            hdc,
            "Lightweight local control panel",
            rect(116, 52, client.right - 24, 76),
            self.fonts.small,
            rgb(160, 184, 220),
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );

        draw_text(
            hdc,
            "Access Link",
            rect(44, 128, client.right - 44, 146),
            self.fonts.caption,
            rgb(125, 176, 255),
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
        let link_color = if self.hover() == HoverTarget::Link {
            rgb(255, 255, 255)
        } else {
            rgb(198, 226, 255)
        };
        draw_text(
            hdc,
            &self.controller.dashboard_url(),
            self.link_rect(),
            self.fonts.mono,
            link_color,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );

        draw_text(
            hdc,
            "Panel Status",
            rect(44, 224, 200, 242),
            self.fonts.caption,
            rgb(125, 176, 255),
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );

        let mut status_color = rgb(24, 194, 118);
        let mut status_text = "Running";
        if !self.controller.is_running() {
            status_color = rgb(229, 96, 96);
            status_text = "Stopped";
        }
        if self.is_busy() {
            status_color = rgb(248, 188, 62);
            status_text = "Processing...";
        }
        fill_rect(hdc, rect(44, 250, 58, 264), status_color);
        draw_text(
            hdc,
            status_text,
            rect(68, 244, 198, 268),
            self.fonts.body,
            rgb(240, 245, 255),
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );

        draw_text(
            hdc,
            "Quick Actions",
            rect(264, 224, client.right - 44, 242),
            self.fonts.caption,
            rgb(125, 176, 255),
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );

        let start_fill = if self.is_busy() {
            rgb(113, 120, 138)
        } else if self.controller.is_running() {
            rgb(220, 38, 38)
        } else {
            rgb(22, 163, 74)
        };
        draw_button(
            hdc,
            self.start_stop_rect(),
            if self.is_busy() {
                "Working..."
            } else if self.controller.is_running() {
                "Stop"
            } else {
                "Start"
            },
            self.fonts.button,
            start_fill,
            darken(start_fill, 22),
            self.hover() == HoverTarget::StartStop,
        );
        draw_button(
            hdc,
            self.hide_rect(),
            "Hide",
            self.fonts.button,
            rgb(69, 118, 255),
            rgb(40, 87, 219),
            self.hover() == HoverTarget::Hide,
        );
        draw_button(
            hdc,
            self.exit_rect(),
            "Exit",
            self.fonts.button,
            rgb(27, 154, 143),
            rgb(17, 125, 116),
            self.hover() == HoverTarget::Exit,
        );

        unsafe {
            EndPaint(hwnd, &paint);
        }
    }

    fn on_mouse_move(&self, point: POINT) {
        let next_hover = self.hit_test(point);
        let mut needs_invalidate = false;

        if let Ok(mut state) = self.state.lock() {
            if !state.tracking_mouse {
                state.tracking_mouse = true;
                let mut tracker = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: self.hwnd(),
                    dwHoverTime: 0,
                };
                unsafe {
                    TrackMouseEvent(&mut tracker);
                }
            }

            if state.hover != next_hover {
                state.hover = next_hover;
                needs_invalidate = true;
            }
        }

        if needs_invalidate {
            self.invalidate();
        }
    }

    fn on_mouse_leave(&self) {
        let mut needs_invalidate = false;
        if let Ok(mut state) = self.state.lock() {
            state.tracking_mouse = false;
            if state.hover != HoverTarget::None {
                state.hover = HoverTarget::None;
                needs_invalidate = true;
            }
        }

        if needs_invalidate {
            self.invalidate();
        }
    }

    fn on_click(self: &Arc<Self>, point: POINT) {
        match self.hit_test(point) {
            HoverTarget::Link => self.open_dashboard(),
            HoverTarget::StartStop => {
                let shell = self.clone();
                shell.run_async(move |controller| {
                    if controller.is_running() {
                        controller.stop()
                    } else {
                        controller.start()
                    }
                });
            }
            HoverTarget::Hide => self.hide_to_tray(),
            HoverTarget::Exit => unsafe {
                DestroyWindow(self.hwnd());
            },
            HoverTarget::None => {}
        }
    }

    fn apply_cursor(&self) -> bool {
        let cursor = unsafe {
            if self.hover() == HoverTarget::Link {
                LoadCursorW(null_mut(), IDC_HAND)
            } else {
                LoadCursorW(null_mut(), IDC_ARROW)
            }
        };

        if !cursor.is_null() {
            unsafe {
                SetCursor(cursor);
            }
            true
        } else {
            false
        }
    }

    fn run_async<F>(self: Arc<Self>, action: F)
    where
        F: FnOnce(Arc<ServerController>) -> Result<(), String> + Send + 'static,
    {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }

        self.invalidate();
        let controller = self.controller.clone();
        thread::spawn(move || {
            let result = action(controller);
            if let Err(error) = result {
                if let Ok(mut state) = self.state.lock() {
                    state.last_error = Some(error);
                }
                unsafe {
                    PostMessageW(self.hwnd(), WM_APP_ERR, 0, 0);
                }
                return;
            }

            unsafe {
                PostMessageW(self.hwnd(), WM_APP_SYNC, 0, 0);
            }
        });
    }

    fn finish_async(&self) {
        self.busy.store(false, Ordering::SeqCst);
        self.invalidate();
    }

    fn show_last_error(&self) {
        let message = self
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.last_error.take());

        if let Some(message) = message {
            show_message_box(self.hwnd(), "MinPanel", &message, MB_OK | MB_ICONERROR);
        }
    }

    fn open_dashboard(&self) {
        let verb = to_wide("open");
        let url = to_wide(&self.controller.dashboard_url());
        unsafe {
            ShellExecuteW(
                self.hwnd(),
                verb.as_ptr(),
                url.as_ptr(),
                null(),
                null(),
                SW_SHOW,
            );
        }
    }

    fn show_tray_menu(&self) {
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return;
        }

        let show_text = to_wide("Show");
        let open_text = to_wide("Open Dashboard");
        let exit_text = to_wide("Exit");

        unsafe {
            AppendMenuW(menu, MF_STRING, MENU_SHOW_ID, show_text.as_ptr());
            AppendMenuW(menu, MF_STRING, MENU_OPEN_ID, open_text.as_ptr());
            AppendMenuW(menu, MF_STRING, MENU_EXIT_ID, exit_text.as_ptr());
            set_tray_menu_item_bitmap(menu, MENU_SHOW_ID, self.menu_icons.show);
            set_tray_menu_item_bitmap(menu, MENU_OPEN_ID, self.menu_icons.open);
            set_tray_menu_item_bitmap(menu, MENU_EXIT_ID, self.menu_icons.exit);
        }

        let mut cursor = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut cursor);
            SetForegroundWindow(self.hwnd());
        }

        let command = unsafe {
            TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
                cursor.x,
                cursor.y,
                0,
                self.hwnd(),
                null(),
            )
        };

        unsafe {
            DestroyMenu(menu);
        }

        match command as usize {
            MENU_SHOW_ID => self.restore_from_tray(),
            MENU_OPEN_ID => self.open_dashboard(),
            MENU_EXIT_ID => unsafe {
                DestroyWindow(self.hwnd());
            },
            _ => {}
        }
    }

    fn hide_to_tray(&self) {
        if !self.add_tray_icon() {
            return;
        }

        if let Ok(mut state) = self.state.lock() {
            state.hover = HoverTarget::None;
        }

        unsafe {
            ShowWindow(self.hwnd(), SW_HIDE);
        }
    }

    fn restore_from_tray(&self) {
        self.remove_tray_icon();
        unsafe {
            ShowWindow(self.hwnd(), SW_SHOW);
            ShowWindow(self.hwnd(), SW_RESTORE);
            SetForegroundWindow(self.hwnd());
            UpdateWindow(self.hwnd());
        }
    }

    fn add_tray_icon(&self) -> bool {
        if let Ok(state) = self.state.lock() {
            if state.tray_visible {
                return true;
            }
        }

        let mut data = self.new_tray_icon_data();
        let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &mut data) };
        if ok == 0 {
            return false;
        }

        if let Ok(mut state) = self.state.lock() {
            state.tray_visible = true;
        }
        true
    }

    fn remove_tray_icon(&self) {
        let mut should_remove = false;
        if let Ok(mut state) = self.state.lock() {
            should_remove = state.tray_visible;
            state.tray_visible = false;
        }
        if !should_remove {
            return;
        }

        let mut data = self.new_tray_icon_data();
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &mut data);
        }
    }

    fn new_tray_icon_data(&self) -> NOTIFYICONDATAW {
        let mut data = unsafe { zeroed::<NOTIFYICONDATAW>() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.hwnd();
        data.uID = TRAY_ICON_ID;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_TRAY_ICON;
        data.hIcon = self.app_icon as _;
        copy_wide_buffer(&mut data.szTip, WINDOW_TITLE);
        data
    }

    fn release_resources(&self) {
        self.remove_tray_icon();
        for bitmap in [
            self.menu_icons.show,
            self.menu_icons.open,
            self.menu_icons.exit,
        ] {
            if bitmap != 0 {
                unsafe {
                    DeleteObject(bitmap as HGDIOBJ);
                }
            }
        }
        for font in [
            self.fonts.title,
            self.fonts.body,
            self.fonts.small,
            self.fonts.mono,
            self.fonts.button,
            self.fonts.caption,
        ] {
            if font != 0 {
                unsafe {
                    DeleteObject(font as HGDIOBJ);
                }
            }
        }
    }

    fn invalidate(&self) {
        unsafe {
            InvalidateRect(self.hwnd(), null(), 1);
        }
    }

    fn client_rect(&self) -> RECT {
        let mut rect = unsafe { zeroed::<RECT>() };
        unsafe {
            GetClientRect(self.hwnd(), &mut rect);
        }
        rect
    }

    fn link_rect(&self) -> RECT {
        let client = self.client_rect();
        rect(44, 154, client.right - 44, 174)
    }

    fn quick_actions_rect(&self) -> RECT {
        let client = self.client_rect();
        rect(248, 212, client.right - 28, 292)
    }

    fn start_stop_rect(&self) -> RECT {
        let card = self.quick_actions_rect();
        let padding = 16;
        let gap = 12;
        let top = card.top + 36;
        let width = ((card.right - card.left) - (padding * 2) - (gap * 2)) / 3;
        let left = card.left + padding;
        rect(left, top, left + width, top + 36)
    }

    fn hide_rect(&self) -> RECT {
        let start = self.start_stop_rect();
        let gap = 12;
        let width = start.right - start.left;
        let left = start.right + gap;
        rect(left, start.top, left + width, start.bottom)
    }

    fn exit_rect(&self) -> RECT {
        let hide = self.hide_rect();
        let gap = 12;
        let width = hide.right - hide.left;
        let left = hide.right + gap;
        rect(left, hide.top, left + width, hide.bottom)
    }

    fn hit_test(&self, point: POINT) -> HoverTarget {
        if contains(point, self.link_rect()) {
            return HoverTarget::Link;
        }
        if contains(point, self.start_stop_rect()) {
            return HoverTarget::StartStop;
        }
        if contains(point, self.hide_rect()) {
            return HoverTarget::Hide;
        }
        if contains(point, self.exit_rect()) {
            return HoverTarget::Exit;
        }
        HoverTarget::None
    }
}

fn create_main_window() -> Result<HWND, String> {
    let instance = unsafe { GetModuleHandleW(null()) };
    if instance.is_null() {
        return Err("Failed to get module handle.".to_string());
    }

    let class_name = to_wide(WINDOW_CLASS_NAME);
    let title = to_wide(WINDOW_TITLE);
    let cursor = unsafe { LoadCursorW(null_mut(), IDC_ARROW) };
    let icon = load_embedded_app_icon();

    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance as HINSTANCE,
        hIcon: icon as _,
        hCursor: cursor,
        lpszClassName: class_name.as_ptr(),
        ..unsafe { zeroed() }
    };

    unsafe {
        RegisterClassW(&class);
    }

    let x = centered_position(unsafe { GetSystemMetrics(0) }, WINDOW_WIDTH);
    let y = centered_position(unsafe { GetSystemMetrics(1) }, WINDOW_HEIGHT);
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE,
            if x > 0 { x } else { CW_USEDEFAULT },
            if y > 0 { y } else { CW_USEDEFAULT },
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        )
    };

    if hwnd.is_null() {
        Err("Failed to create Win32 control panel window.".to_string())
    } else {
        if icon != 0 {
            unsafe {
                SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon);
                SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon);
            }
        }
        Ok(hwnd)
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(shell) = clone_active_shell() else {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    };

    match message {
        WM_CREATE => {
            shell.set_hwnd(hwnd);
            0
        }
        WM_PAINT => {
            shell.paint();
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEMOVE => {
            shell.on_mouse_move(point_from_lparam(lparam));
            0
        }
        WM_MOUSELEAVE => {
            shell.on_mouse_leave();
            0
        }
        WM_LBUTTONUP => {
            shell.on_click(point_from_lparam(lparam));
            0
        }
        WM_COMMAND => {
            match loword(wparam as usize) as usize {
                MENU_SHOW_ID => shell.restore_from_tray(),
                MENU_OPEN_ID => shell.open_dashboard(),
                MENU_EXIT_ID => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_TRAY_ICON => {
            match lparam as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => shell.restore_from_tray(),
                WM_RBUTTONUP => shell.show_tray_menu(),
                _ => {}
            }
            0
        }
        WM_SETCURSOR => {
            if shell.apply_cursor() {
                1
            } else {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        WM_APP_SYNC => {
            shell.finish_async();
            0
        }
        WM_APP_ERR => {
            shell.finish_async();
            shell.show_last_error();
            0
        }
        WM_APP_RESTORE => {
            shell.restore_from_tray();
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            shell.release_resources();
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn clone_active_shell() -> Option<Arc<ControlPanelShell>> {
    ACTIVE_SHELL
        .get()
        .and_then(|slot| slot.lock().ok().and_then(|shell| shell.clone()))
}

fn set_active_shell(shell: Arc<ControlPanelShell>) {
    let slot = ACTIVE_SHELL.get_or_init(|| Mutex::new(None));
    if let Ok(mut current) = slot.lock() {
        *current = Some(shell);
    }
}

fn clear_active_shell() {
    if let Some(slot) = ACTIVE_SHELL.get() {
        if let Ok(mut current) = slot.lock() {
            *current = None;
        }
    }
}

fn load_embedded_app_icon() -> isize {
    let instance = unsafe { GetModuleHandleW(null()) };
    if instance.is_null() {
        return unsafe { LoadIconW(null_mut(), IDI_APPLICATION) as isize };
    }

    let icon = unsafe { LoadIconW(instance, resource_id(1)) };
    if icon.is_null() {
        unsafe { LoadIconW(null_mut(), IDI_APPLICATION) as isize }
    } else {
        icon as isize
    }
}

#[derive(Clone, Copy)]
enum TrayMenuIconKind {
    Show,
    Open,
    Exit,
}

unsafe fn set_tray_menu_item_bitmap(menu: *mut core::ffi::c_void, id: usize, bitmap: isize) {
    if menu.is_null() || bitmap == 0 {
        return;
    }

    let _ = SetMenuItemBitmaps(menu, id as u32, MF_BYCOMMAND, bitmap as _, null_mut());
}

fn create_tray_menu_icon(kind: TrayMenuIconKind, color: COLORREF) -> isize {
    const ICON_SIZE: i32 = 16;

    let screen_dc = unsafe { GetDC(null_mut()) };
    if screen_dc.is_null() {
        return 0;
    }

    let bitmap = unsafe { CreateCompatibleBitmap(screen_dc, ICON_SIZE, ICON_SIZE) };
    let mem_dc = unsafe { CreateCompatibleDC(screen_dc) };
    unsafe {
        ReleaseDC(null_mut(), screen_dc);
    }

    if bitmap.is_null() || mem_dc.is_null() {
        if !bitmap.is_null() {
            unsafe {
                DeleteObject(bitmap as HGDIOBJ);
            }
        }
        if !mem_dc.is_null() {
            unsafe {
                DeleteDC(mem_dc);
            }
        }
        return 0;
    }

    let old_bitmap = unsafe { SelectObject(mem_dc, bitmap as HGDIOBJ) };
    fill_rect(mem_dc, rect(0, 0, ICON_SIZE, ICON_SIZE), rgb(255, 255, 255));

    let pen = unsafe { CreatePen(PS_SOLID, 2, color) };
    if !pen.is_null() {
        let old_pen = unsafe { SelectObject(mem_dc, pen as HGDIOBJ) };
        match kind {
            TrayMenuIconKind::Show => {
                draw_icon_line(mem_dc, 3, 3, 12, 3);
                draw_icon_line(mem_dc, 12, 3, 12, 10);
                draw_icon_line(mem_dc, 12, 10, 3, 10);
                draw_icon_line(mem_dc, 3, 10, 3, 3);
                draw_icon_line(mem_dc, 3, 5, 12, 5);
            }
            TrayMenuIconKind::Open => {
                draw_icon_line(mem_dc, 3, 7, 9, 7);
                draw_icon_line(mem_dc, 9, 7, 9, 12);
                draw_icon_line(mem_dc, 9, 12, 3, 12);
                draw_icon_line(mem_dc, 3, 12, 3, 7);
                draw_icon_line(mem_dc, 7, 9, 12, 4);
                draw_icon_line(mem_dc, 9, 4, 12, 4);
                draw_icon_line(mem_dc, 12, 4, 12, 7);
            }
            TrayMenuIconKind::Exit => {
                draw_icon_line(mem_dc, 4, 4, 12, 12);
                draw_icon_line(mem_dc, 12, 4, 4, 12);
            }
        }
        unsafe {
            SelectObject(mem_dc, old_pen);
            DeleteObject(pen as HGDIOBJ);
        }
    }

    unsafe {
        SelectObject(mem_dc, old_bitmap);
        DeleteDC(mem_dc);
    }

    bitmap as isize
}

fn draw_icon_line(hdc: HDC, x1: i32, y1: i32, x2: i32, y2: i32) {
    unsafe {
        MoveToEx(hdc, x1, y1, null_mut());
        LineTo(hdc, x2, y2);
    }
}

fn resource_id(id: u16) -> *const u16 {
    id as usize as *const u16
}

fn create_font(height: i32, weight: i32, name: &str) -> isize {
    let face = to_wide(name);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face.as_ptr(),
        ) as isize
    }
}

fn draw_button(
    hdc: HDC,
    bounds: RECT,
    label: &str,
    font: isize,
    fill: COLORREF,
    border: COLORREF,
    hovered: bool,
) {
    fill_rect(hdc, bounds, border);
    let inner = rect(
        bounds.left + 1,
        bounds.top + 1,
        bounds.right - 1,
        bounds.bottom - 1,
    );
    fill_rect(hdc, inner, if hovered { lighten(fill, 12) } else { fill });

    let mut label_rect = inner;
    if hovered {
        label_rect.top -= 1;
        label_rect.bottom -= 1;
    }

    draw_text(
        hdc,
        label,
        label_rect,
        font,
        rgb(255, 255, 255),
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
}

fn draw_text(hdc: HDC, value: &str, mut bounds: RECT, font: isize, color: COLORREF, flags: u32) {
    let text = to_wide(value);
    unsafe {
        let old_font = SelectObject(hdc, font as HGDIOBJ);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, color);
        DrawTextW(hdc, text.as_ptr() as *mut u16, -1, &mut bounds, flags);
        SelectObject(hdc, old_font);
    }
}

fn fill_rect(hdc: HDC, bounds: RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(hdc, &bounds, brush as HBRUSH);
        DeleteObject(brush as HGDIOBJ);
    }
}

fn show_message_box(hwnd: HWND, title: &str, message: &str, flags: u32) {
    let title_w = to_wide(title);
    let message_w = to_wide(message);
    unsafe {
        MessageBoxW(hwnd, message_w.as_ptr(), title_w.as_ptr(), flags);
    }
}

fn point_from_lparam(value: LPARAM) -> POINT {
    POINT {
        x: (value as i16) as i32,
        y: ((value >> 16) as i16) as i32,
    }
}

fn loword(value: usize) -> u16 {
    (value & 0xffff) as u16
}

fn contains(point: POINT, bounds: RECT) -> bool {
    point.x >= bounds.left
        && point.x <= bounds.right
        && point.y >= bounds.top
        && point.y <= bounds.bottom
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

fn centered_position(screen_size: i32, window_size: i32) -> i32 {
    (screen_size - window_size) / 2
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16)
}

fn darken(color: COLORREF, delta: u8) -> COLORREF {
    rgb(
        clamp_channel((color & 0xff) as i32 - i32::from(delta)),
        clamp_channel(((color >> 8) & 0xff) as i32 - i32::from(delta)),
        clamp_channel(((color >> 16) & 0xff) as i32 - i32::from(delta)),
    )
}

fn lighten(color: COLORREF, delta: u8) -> COLORREF {
    rgb(
        clamp_channel((color & 0xff) as i32 + i32::from(delta)),
        clamp_channel(((color >> 8) & 0xff) as i32 + i32::from(delta)),
        clamp_channel(((color >> 16) & 0xff) as i32 + i32::from(delta)),
    )
}

fn clamp_channel(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

fn to_wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn copy_wide_buffer<const N: usize>(buffer: &mut [u16; N], value: &str) {
    let encoded = to_wide(value);
    for (index, unit) in encoded.into_iter().take(N.saturating_sub(1)).enumerate() {
        buffer[index] = unit;
    }
}

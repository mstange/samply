use ::windows::core::{w, PCWSTR};
use ::windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use ::windows::Win32::Graphics::Gdi::{GetStockObject, DEFAULT_GUI_FONT, HBRUSH, HFONT};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::Controls::{
    ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, GetClientRect, GetMessageW, GetWindowLongPtrW, GWLP_USERDATA, IDC_ARROW,
    LoadCursorW, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SW_HIDE, SW_SHOW,
    SendMessageW, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
    SWP_NOMOVE, SWP_NOZORDER, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    HMENU, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COMMAND, WM_CREATE, WM_DESTROY,
    WM_SETFONT, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_OVERLAPPED, WS_SYSMENU, WS_VISIBLE,
};

/// Stores the profile path produced by the UI recording thread so the main
/// thread can serve it after the window closes.
static UI_RESULT_PATH: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

const CLASS_NAME: PCWSTR = w!("SamplyWindowClass");
const RECORD_BUTTON_ID: usize = 1001;
const CONFIGURE_BTN_ID: usize = 1002;
const BROWSERS_CHECK_ID: usize = 1003;
const GRAPHICS_CHECK_ID: usize = 1004;
const SYMBOLS_SERVER_CHECK_ID: usize = 1005;
const MOZILLA_SERVER_CHECK_ID: usize = 1006;

const WINDOW_W: i32 = 320;
const COLLAPSED_H: i32 = 140;
const EXPANDED_H: i32 = 315;

unsafe fn create_button(
    parent: HWND, text: PCWSTR, style: WINDOW_STYLE,
    x: i32, y: i32, w: i32, h: i32, id: usize,
) -> Option<HWND> {
    CreateWindowExW(
        WINDOW_EX_STYLE(0), w!("BUTTON"), text, style,
        x, y, w, h,
        Some(parent), Some(HMENU(id as *mut core::ffi::c_void)), None, None,
    ).ok()
}

unsafe fn create_static(
    parent: HWND, text: PCWSTR, style: WINDOW_STYLE,
    x: i32, y: i32, w: i32, h: i32,
) -> Option<HWND> {
    CreateWindowExW(
        WINDOW_EX_STYLE(0), w!("STATIC"), text, style,
        x, y, w, h,
        Some(parent), None, None, None,
    ).ok()
}

unsafe fn get_state_mut(hwnd: HWND) -> Option<&'static mut UiState> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut UiState).as_mut()
}

unsafe fn is_checked(hwnd: Option<HWND>) -> bool {
    hwnd.is_some_and(|h| SendMessageW(h, BM_GETCHECK, None, None).0 == 1)
}

// Wraps an HWND as a usize so it can be sent across threads.
// HWND is process-wide and safe to use across threads for PostMessageW.
struct SendHwnd(usize);
unsafe impl Send for SendHwnd {}

struct UiState {
    stop_tx: Option<std::sync::mpsc::SyncSender<()>>,
    button_hwnd: Option<HWND>,
    configure_expanded: bool,
    configure_btn: Option<HWND>,
    providers_label: Option<HWND>,
    browsers_check: Option<HWND>,
    graphics_check: Option<HWND>,
    symbols_label: Option<HWND>,
    symbols_server_check: Option<HWND>,
    mozilla_server_check: Option<HWND>,
}

// The window has two sections:
//   - Always visible: "Start Recording" button and a "More options" toggle.
//   - Collapsible area with the additional options
//
// Overall flow:
//   WM_COMMAND/RECORD_BUTTON_ID → spawn recording thread, pass a SyncSender
//     so the thread blocks until stop is requested.
//   Second click → drop the SyncSender, unblocking the thread.
//   Recording thread → saves the profile, then posts WM_APP back to the window.
//   WM_APP → resets the button, reads symbol-server checkboxes, spawns a
//     server thread that opens the finished profile in the browser.
//
// UiState is heap-allocated and stored in the window's GWLP_USERDATA slot;
// WM_DESTROY drops it.
#[allow(non_snake_case)]
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let client_w = rc.right - rc.left;
            let btn_w = 140i32;
            let cfg_btn_w = 130i32;
            let btn_x = (client_w - btn_w) / 2;
            let cfg_btn_x = (client_w - cfg_btn_w) / 2;

            let vis_btn = WINDOW_STYLE((WS_CHILD | WS_VISIBLE).0 | BS_PUSHBUTTON as u32);
            let button_hwnd = create_button(hwnd, w!("Start Recording"), vis_btn, btn_x, 14, btn_w, 32, RECORD_BUTTON_ID);
            let configure_btn = create_button(hwnd, w!("More options \u{25BC}"), vis_btn, cfg_btn_x, 58, cfg_btn_w, 24, CONFIGURE_BTN_ID);

            // Config section controls start hidden (no WS_VISIBLE).
            let hidden = WINDOW_STYLE(WS_CHILD.0);
            let hidden_check = WINDOW_STYLE(WS_CHILD.0 | BS_AUTOCHECKBOX as u32);
            let providers_label    = create_static(hwnd, w!("Providers:"), hidden, 15, 96, 100, 20);
            let browsers_check     = create_button(hwnd, w!("Browsers"), hidden_check, 25, 120, 150, 20, BROWSERS_CHECK_ID);
            let graphics_check     = create_button(hwnd, w!("Graphics"), hidden_check, 25, 145, 150, 20, GRAPHICS_CHECK_ID);
            let symbols_label      = create_static(hwnd, w!("Symbols:"), hidden, 15, 175, 100, 20);
            let symbols_server_check = create_button(hwnd, w!("Use Microsoft symbol server"), hidden_check, 25, 200, 250, 20, SYMBOLS_SERVER_CHECK_ID);
            let mozilla_server_check = create_button(hwnd, w!("Use Mozilla symbol server"), hidden_check, 25, 225, 250, 20, MOZILLA_SERVER_CHECK_ID);

            let font = HFONT(GetStockObject(DEFAULT_GUI_FONT).0);
            if !font.is_invalid() {
                for ctrl in [button_hwnd, configure_btn, providers_label, browsers_check,
                             graphics_check, symbols_label, symbols_server_check, mozilla_server_check] {
                    if let Some(h) = ctrl {
                        let _ = SendMessageW(h, WM_SETFONT,
                            Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
                    }
                }
            }

            let state = Box::new(UiState {
                stop_tx: None,
                button_hwnd,
                configure_expanded: false,
                configure_btn,
                providers_label,
                browsers_check,
                graphics_check,
                symbols_label,
                symbols_server_check,
                mozilla_server_check,
            });
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
            LRESULT(0)
        }
        WM_COMMAND => {
            let control_id = (wparam.0 & 0xFFFF) as usize;
            let notification = ((wparam.0 >> 16) & 0xFFFF) as u32;

            if control_id == CONFIGURE_BTN_ID && notification == BN_CLICKED {
                let Some(state) = get_state_mut(hwnd) else { return LRESULT(0); };
                state.configure_expanded = !state.configure_expanded;
                let expanded = state.configure_expanded;

                let show_cmd = if expanded { SW_SHOW } else { SW_HIDE };
                let label = if expanded { w!("Fewer options \u{25B2}") } else { w!("More options \u{25BC}") };

                for ctrl in [state.providers_label, state.browsers_check,
                             state.graphics_check, state.symbols_label,
                             state.symbols_server_check, state.mozilla_server_check] {
                    if let Some(h) = ctrl {
                        let _ = ShowWindow(h, show_cmd);
                    }
                }

                if let Some(btn) = state.configure_btn {
                    let _ = SetWindowTextW(btn, label);
                }

                let new_h = if expanded { EXPANDED_H } else { COLLAPSED_H };
                let _ = SetWindowPos(hwnd, None, 0, 0, WINDOW_W, new_h,
                    SWP_NOMOVE | SWP_NOZORDER);
                return LRESULT(0);
            }

            if control_id == RECORD_BUTTON_ID && notification == BN_CLICKED {
                let Some(state) = get_state_mut(hwnd) else { return LRESULT(0); };

                if state.stop_tx.is_some() {
                    // Drop the sender to unblock the recording thread's recv().
                    state.stop_tx = None;
                    if let Some(btn) = state.button_hwnd {
                        let _ = SetWindowTextW(btn, w!("Processing..."));
                    }
                } else {
                    // Read checkbox states before spawning the recording thread.
                    let gfx = is_checked(state.graphics_check);
                    let browsers = is_checked(state.browsers_check);
                    let unknown_event_markers = gfx;

                    // Start recording on a background thread.
                    let (stop_tx, stop_rx) = std::sync::mpsc::sync_channel::<()>(0);
                    state.stop_tx = Some(stop_tx);

                    let send_hwnd = SendHwnd(hwnd.0 as usize);
                    let output_path = std::env::temp_dir().join("samply-profile.json.gz");

                    std::thread::spawn(move || {
                        use crate::shared::prop_types::{
                            CoreClrProfileProps, ProfileCreationProps, RecordingMode,
                            RecordingProps,
                        };

                        let recording_props = RecordingProps {
                            output_file: output_path.clone(),
                            time_limit: None,
                            interval: std::time::Duration::from_millis(1),
                            vm_hack: false,
                            gfx,
                            browsers,
                            keep_etl: false,
                        };
                        let profile_creation_props = ProfileCreationProps {
                            profile_name: None,
                            fallback_profile_name: "UI Recording".to_string(),
                            main_thread_only: false,
                            reuse_threads: false,
                            fold_recursive_prefix: false,
                            unlink_aux_files: false,
                            create_per_cpu_threads: false,
                            arg_count_to_include_in_process_name: 0,
                            override_arch: None,
                            presymbolicate: false,
                            coreclr: CoreClrProfileProps::default(),
                            unknown_event_markers,
                            should_emit_jit_markers: false,
                            should_emit_cswitch_markers: false,
                        };

                        let success = match super::profiler::run(
                            RecordingMode::All,
                            recording_props,
                            profile_creation_props,
                            Some(stop_rx),
                        ) {
                            Ok((profile, _)) => {
                                crate::shared::save_profile::save_profile_to_file(
                                    &profile,
                                    &output_path,
                                )
                                .is_ok()
                            }
                            Err(_) => false,
                        };

                        if success {
                            *UI_RESULT_PATH.lock().unwrap() = Some(output_path);
                        }

                        let hwnd = HWND(send_hwnd.0 as *mut std::ffi::c_void);
                        let _ = PostMessageW(Some(hwnd), WM_APP, WPARAM(0), LPARAM(0));
                    });

                    if let Some(btn) = state.button_hwnd {
                        let _ = SetWindowTextW(btn, w!("Stop Recording"));
                    }
                }

                return LRESULT(0);
            }

            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_APP => {
            // Reset button so the user can record again.
            let mut use_ms_symbols = false;
            let mut use_mozilla_symbols = false;
            if let Some(state) = get_state_mut(hwnd) {
                if let Some(btn) = state.button_hwnd {
                    let _ = SetWindowTextW(btn, w!("Start Recording"));
                }
                use_ms_symbols = is_checked(state.symbols_server_check);
                use_mozilla_symbols = is_checked(state.mozilla_server_check);
            }
            // Open the profile in the browser on a background thread.
            let profile_path = UI_RESULT_PATH.lock().unwrap().take();
            if let Some(path) = profile_path {
                let mut windows_symbol_server = Vec::new();
                if use_ms_symbols {
                    windows_symbol_server.push("https://msdl.microsoft.com/download/symbols".to_string());
                }
                if use_mozilla_symbols {
                    windows_symbol_server.push("https://symbols.mozilla.org/".to_string());
                }
                std::thread::spawn(move || {
                    crate::run_server_serving_profile(
                        &path,
                        crate::server::ServerProps {
                            address: "127.0.0.1".parse().unwrap(),
                            port_selection: crate::server::PortSelection::TryMultiple(3000..3100),
                            verbose: false,
                            open_in_browser: true,
                        },
                        crate::shared::prop_types::SymbolProps {
                            symbol_dir: Vec::new(),
                            windows_symbol_server,
                            windows_symbol_cache: None,
                            breakpad_symbol_server: Vec::new(),
                            breakpad_symbol_dir: Vec::new(),
                            breakpad_symbol_cache: None,
                            simpleperf_binary_cache: None,
                        },
                    );
                });
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut UiState;
            if !state_ptr.is_null() {
                drop(Box::from_raw(state_ptr));
                let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn run() {
    let result = (|| -> ::windows::core::Result<()> { unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);

        let instance = GetModuleHandleW(None)?;

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH((15i32 + 1) as usize as *mut core::ffi::c_void), // COLOR_BTNFACE + 1
            ..Default::default()
        };

        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("Samply"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WINDOW_W,
            COLLAPSED_H,
            None,
            None,
            Some(instance.into()),
            None,
        )?;

        let _ = ShowWindow(hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        Ok(())
    } })();

    if let Err(e) = result {
        eprintln!("UI error: {e}");
    }
}

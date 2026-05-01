use std::cell::{Cell, RefCell};
use std::rc::Rc;
use winsafe::{self as w, co, gui, prelude::*};

use crate::shared::StopCondition;
use crate::windows::elevated_helper::run_elevated_helper;

static UI_RESULT_PATH: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

const WINDOW_W: i32 = 320;
const COLLAPSED_H: i32 = 120;
const EXPANDED_H: i32 = 280;

pub fn run() {
    use crate::cli::RunElevatedHelperArgs;
    use clap::{Parser, Subcommand};

    #[derive(Debug, Parser)]
    #[command(name = "samply", version)]
    pub struct Opt {
        #[command(subcommand)]
        pub action: Option<Action>,
    }

    #[derive(Debug, Subcommand)]
    pub enum Action {
        #[clap(hide = true)]
        /// Used in the elevated helper process.
        RunElevatedHelper(RunElevatedHelperArgs),
    }
    match Opt::parse() {
        Opt {
            action: Some(Action::RunElevatedHelper(args)),
        } => {
            run_elevated_helper(&args.ipc_directory, args.output_path);
            return;
        }
        _ => {}
    }
    if let Err(e) = run_inner() {
        eprintln!("UI error: {e}");
    }
}

fn run_inner() -> w::AnyResult<i32> {
    let btn_x = (WINDOW_W - 140) / 2;
    let cfg_btn_x = (WINDOW_W - 130) / 2;
    let hidden = co::WS::CHILD;

    let wnd = gui::WindowMain::new(gui::WindowMainOpts {
        title: "Samply",
        class_style: co::CS::HREDRAW | co::CS::VREDRAW,
        size: (WINDOW_W, COLLAPSED_H),
        style: co::WS::OVERLAPPED | co::WS::CAPTION | co::WS::SYSMENU | co::WS::VISIBLE,
        class_bg_brush: gui::Brush::Color(co::COLOR::BTNFACE),
        ..Default::default()
    });

    let record_btn = gui::Button::new(
        &wnd,
        gui::ButtonOpts {
            text: "Start Recording",
            position: (btn_x, 14),
            width: 140,
            height: 32,
            ..Default::default()
        },
    );

    let configure_btn = gui::Button::new(
        &wnd,
        gui::ButtonOpts {
            text: "More options \u{25BC}",
            position: (cfg_btn_x, 58),
            width: 130,
            height: 24,
            ..Default::default()
        },
    );

    let providers_label = gui::Label::new(
        &wnd,
        gui::LabelOpts {
            text: "Providers:",
            position: (15, 96),
            size: (100, 20),
            window_style: hidden,
            ..Default::default()
        },
    );

    let browsers_check = gui::CheckBox::new(
        &wnd,
        gui::CheckBoxOpts {
            text: "Browsers",
            position: (25, 120),
            size: (150, 20),
            window_style: co::WS::CHILD | co::WS::GROUP | co::WS::TABSTOP,
            ..Default::default()
        },
    );

    let graphics_check = gui::CheckBox::new(
        &wnd,
        gui::CheckBoxOpts {
            text: "Graphics",
            position: (25, 145),
            size: (150, 20),
            window_style: co::WS::CHILD | co::WS::GROUP | co::WS::TABSTOP,
            ..Default::default()
        },
    );

    let symbols_label = gui::Label::new(
        &wnd,
        gui::LabelOpts {
            text: "Symbols:",
            position: (15, 175),
            size: (100, 20),
            window_style: hidden,
            ..Default::default()
        },
    );

    let symbols_server_check = gui::CheckBox::new(
        &wnd,
        gui::CheckBoxOpts {
            text: "Use Microsoft symbol server",
            position: (25, 200),
            size: (250, 20),
            window_style: co::WS::CHILD | co::WS::GROUP | co::WS::TABSTOP,
            ..Default::default()
        },
    );

    let mozilla_server_check = gui::CheckBox::new(
        &wnd,
        gui::CheckBoxOpts {
            text: "Use Mozilla symbol server",
            position: (25, 225),
            size: (250, 20),
            window_style: co::WS::CHILD | co::WS::GROUP | co::WS::TABSTOP,
            ..Default::default()
        },
    );

    let configure_expanded = Rc::new(Cell::new(false));
    let stop_tx: Rc<RefCell<Option<std::sync::mpsc::SyncSender<()>>>> = Rc::new(RefCell::new(None));

    configure_btn.on().bn_clicked({
        let wnd = wnd.clone();
        let configure_btn = configure_btn.clone();
        let providers_label = providers_label.clone();
        let browsers_check = browsers_check.clone();
        let graphics_check = graphics_check.clone();
        let symbols_label = symbols_label.clone();
        let symbols_server_check = symbols_server_check.clone();
        let mozilla_server_check = mozilla_server_check.clone();
        let configure_expanded = configure_expanded.clone();
        move || {
            let expanded = !configure_expanded.get();
            configure_expanded.set(expanded);
            let show_cmd = if expanded { co::SW::SHOW } else { co::SW::HIDE };
            let label = if expanded {
                "Fewer options \u{25B2}"
            } else {
                "More options \u{25BC}"
            };

            providers_label.hwnd().ShowWindow(show_cmd);
            browsers_check.hwnd().ShowWindow(show_cmd);
            graphics_check.hwnd().ShowWindow(show_cmd);
            symbols_label.hwnd().ShowWindow(show_cmd);
            symbols_server_check.hwnd().ShowWindow(show_cmd);
            mozilla_server_check.hwnd().ShowWindow(show_cmd);

            configure_btn.hwnd().SetWindowText(label)?;

            let new_h = if expanded { EXPANDED_H } else { COLLAPSED_H };
            let wr = wnd.hwnd().GetWindowRect()?;
            let cr = wnd.hwnd().GetClientRect()?;
            let nc_cx = (wr.right - wr.left) - cr.right;
            let nc_cy = (wr.bottom - wr.top) - cr.bottom;
            wnd.hwnd().SetWindowPos(
                w::HwndPlace::None,
                w::POINT { x: 0, y: 0 },
                w::SIZE {
                    cx: WINDOW_W + nc_cx,
                    cy: new_h + nc_cy,
                },
                co::SWP::NOMOVE | co::SWP::NOZORDER,
            )?;
            Ok(())
        }
    });

    record_btn.on().bn_clicked({
        let wnd = wnd.clone();
        let record_btn = record_btn.clone();
        let graphics_check = graphics_check.clone();
        let browsers_check = browsers_check.clone();
        let stop_tx = stop_tx.clone();
        move || {
            let mut tx = stop_tx.borrow_mut();
            if tx.is_some() {
                *tx = None;
                record_btn.hwnd().SetWindowText("Processing...")?;
            } else {
                let gfx = graphics_check.is_checked();
                let browsers = browsers_check.is_checked();
                let unknown_event_markers = gfx;

                let (sender, rx) = std::sync::mpsc::sync_channel::<()>(0);
                *tx = Some(sender);

                let output_path = std::env::temp_dir().join("samply-profile.json.gz");
                let wnd = wnd.clone();
                std::thread::spawn(move || {
                    use crate::shared::prop_types::{
                        CoreClrProfileProps, ProfileCreationProps, RecordingMode, RecordingProps,
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
                        StopCondition::ReceiverSignaled(rx),
                    ) {
                        Ok((profile, _)) => crate::shared::save_profile::save_profile_to_file(
                            &profile,
                            &output_path,
                        )
                        .is_ok(),
                        Err(_) => false,
                    };
                    if success {
                        *UI_RESULT_PATH.lock().unwrap() = Some(output_path);
                    }
                    let _ = unsafe {
                        wnd.hwnd().PostMessage(w::msg::WndMsg {
                            msg_id: co::WM::APP,
                            wparam: 0,
                            lparam: 0,
                        })
                    };
                });

                record_btn.hwnd().SetWindowText("Stop Recording")?;
            }
            Ok(())
        }
    });

    wnd.on().wm(co::WM::APP, {
        let record_btn = record_btn.clone();
        let symbols_server_check = symbols_server_check.clone();
        let mozilla_server_check = mozilla_server_check.clone();
        move |_| {
            record_btn.hwnd().SetWindowText("Start Recording")?;
            let use_ms_symbols = symbols_server_check.is_checked();
            let use_mozilla_symbols = mozilla_server_check.is_checked();

            let profile_path = UI_RESULT_PATH.lock().unwrap().take();
            if let Some(path) = profile_path {
                let mut windows_symbol_server = Vec::new();
                if use_ms_symbols {
                    windows_symbol_server
                        .push("https://msdl.microsoft.com/download/symbols".to_string());
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
            Ok(0)
        }
    });

    wnd.run_main(None)
}

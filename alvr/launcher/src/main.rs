#![windows_subsystem = "windows"]

mod commands;

use alvr_common::prelude::*;
use alvr_filesystem as afs;
use eframe::egui;
use std::{
    env,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

const WINDOW_WIDTH: f32 = 500.0;
const WINDOW_HEIGHT: f32 = 300.0;

#[derive(Clone, PartialEq)]
enum View {
    RequirementsCheck { steamvr: String },
    Launching { resetting: bool },
}

struct SharedState {
    view: View,
    should_close: bool,
}

fn launcher_lifecycle(state: Arc<Mutex<SharedState>>, ctx: egui::Context) {
    loop {
        if commands::check_steamvr_installation() {
            break;
        }

        state.lock().unwrap().view = View::RequirementsCheck {
            steamvr:
                "SteamVR not installed: make sure you launched it at least once, then close it."
                    .to_owned(),
        };
        ctx.request_repaint();
        thread::sleep(Duration::from_millis(500));
    }

    state.lock().unwrap().view = View::Launching { resetting: false };
    ctx.request_repaint();

    let request_agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_millis(100)))
            .build(),
    );

    let mut tried_steamvr_launch = false;
    loop {
        let maybe_response = request_agent.get("http://127.0.0.1:8082/index.html").call();
        if let Ok(response) = maybe_response
            && response.status().is_success()
        {
            state.lock().unwrap().should_close = true;
            ctx.request_repaint();
            break;
        }

        if !tried_steamvr_launch {
            if alvr_common::show_err(commands::maybe_register_alvr_driver()).is_some() {
                if commands::is_steamvr_running() {
                    commands::kill_steamvr();
                    thread::sleep(Duration::from_secs(2))
                }
                commands::maybe_launch_steamvr();
            }
            tried_steamvr_launch = true;
        }

        thread::sleep(Duration::from_millis(500));
    }
}

fn reset_and_retry(state: Arc<Mutex<SharedState>>, ctx: egui::Context) {
    thread::spawn(move || {
        state.lock().unwrap().view = View::Launching { resetting: true };
        ctx.request_repaint();

        commands::kill_steamvr();
        commands::fix_steamvr();
        commands::restart_steamvr();

        thread::sleep(Duration::from_secs(2));

        state.lock().unwrap().view = View::Launching { resetting: false };
        ctx.request_repaint();
    });
}

struct LauncherApp {
    state: Arc<Mutex<SharedState>>,
    lifecycle_spawned: bool,
}

impl LauncherApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        Self {
            state: Arc::new(Mutex::new(SharedState {
                view: View::RequirementsCheck {
                    steamvr: String::new(),
                },
                should_close: false,
            })),
            lifecycle_spawned: false,
        }
    }
}

impl eframe::App for LauncherApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.lifecycle_spawned {
            self.lifecycle_spawned = true;
            let state = Arc::clone(&self.state);
            let ctx_clone = ctx.clone();
            thread::spawn(move || launcher_lifecycle(state, ctx_clone));
        }

        if self.state.lock().unwrap().should_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let view = self.state.lock().unwrap().view.clone();

        match &view {
            View::RequirementsCheck { steamvr } => {
                ui.add_space(ui.available_height() / 3.0);
                ui.horizontal(|ui| {
                    ui.add_space(35.0);
                    ui.label(steamvr);
                });
            }
            View::Launching { resetting } => {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.label(egui::RichText::new("Waiting for server to load...").size(25.0));
                    ui.add_space(15.0);
                    if *resetting {
                        ui.label("Please wait for multiple restarts");
                    } else if ui.button("Reset drivers and retry").clicked() {
                        reset_and_retry(Arc::clone(&self.state), ui.ctx().clone());
                    }
                });
            }
        }
    }
}

fn make_window() -> StrResult {
    let instance_mutex = trace_err!(single_instance::SingleInstance::new("alvr_launcher_mutex"))?;
    if instance_mutex.is_single() {
        let driver_dir = afs::filesystem_layout_from_launcher_exe(&env::current_exe().unwrap())
            .openvr_driver_root_dir;

        if driver_dir.to_str().filter(|s| s.is_ascii()).is_none() {
            alvr_common::show_e_blocking(format!(
                "The path of this folder ({}) contains non ASCII characters. {}",
                driver_dir.to_string_lossy(),
                "Please move it somewhere else (for example in C:\\Users\\Public\\Documents).",
            ));
            return Ok(());
        }

        let native_options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
                .with_min_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
                .with_resizable(false),
            centered: true,
            ..Default::default()
        };

        trace_err!(eframe::run_native(
            "ALVR Launcher",
            native_options,
            Box::new(|cc| Ok(Box::new(LauncherApp::new(cc)))),
        ))?;
    }
    Ok(())
}

fn main() {
    let args = env::args().collect::<Vec<_>>();
    match args.get(1) {
        Some(flag) if flag == "--restart-steamvr" => commands::restart_steamvr(),
        Some(flag) if flag == "--update" => commands::invoke_installer(),
        Some(_) | None => {
            alvr_common::show_err_blocking(make_window());
        }
    }
}

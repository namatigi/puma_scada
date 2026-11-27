//! Moviclone Studio for Windows and macOS.
//!
//! One process: the Moviclone runtime (`moviclone_rt`: drivers or simulator,
//! historian, alarms, gateway, studio API) runs inside the app on
//! `127.0.0.1`, and the app's window shows the studio from it. The HMI opens
//! in a second window or the browser. Nothing else needs to be installed.
//!
//! - The launcher (`dist/`) lists recent projects and opens, creates or
//!   imports one.
//! - Per project, history, the alarm journal and users live in the app's data
//!   folder (`%APPDATA%\com.moviclone.studio\projects\…` on Windows,
//!   `~/Library/Application Support/com.moviclone.studio/projects/…` on macOS).
//! - The web pages are bundled as resources (`web/`), so it works offline.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod projects;

use projects::Settings;
use serde::Serialize;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, Wry};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

/// Ports tried first, so the studio keeps its origin (and its saved layout)
/// from one start to the next. A random free port if all are taken.
const PREFERRED_PORTS: std::ops::Range<u16> = 47801..47811;
const MAIN: &str = "main";
const HMI: &str = "hmi";

struct Running {
    rt: moviclone_rt::Runtime,
    dir: PathBuf,
    name: String,
    addr: SocketAddr,
    data: PathBuf,
}

struct AppState {
    running: tokio::sync::Mutex<Option<Running>>,
    settings: Mutex<Settings>,
    settings_file: PathBuf,
    /// Serializes project switches (menu clicks and launcher buttons).
    switching: tokio::sync::Mutex<()>,
}

impl AppState {
    fn settings(&self) -> Settings {
        self.settings.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let mut s = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut s);
        s.save(&self.settings_file);
    }
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            launcher_state,
            open_project_dialog,
            open_recent,
            forget_recent,
            new_project,
            import_movicon,
            set_simulate,
            site_template,
            choose_folder,
            create_depot,
            skip_setup
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            init_logging(&handle);
            let config = handle.path().app_config_dir()?;
            let settings_file = config.join("settings.json");
            app.manage(AppState {
                running: tokio::sync::Mutex::new(None),
                settings: Mutex::new(Settings::load(&settings_file)),
                settings_file,
                switching: tokio::sync::Mutex::new(()),
            });
            handle.set_menu(build_menu(&handle)?)?;
            handle.on_menu_event(|app, event| on_menu(app, event.id().as_ref()));
            WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
                .title("Moviclone Studio")
                .inner_size(1440.0, 900.0)
                .min_inner_size(960.0, 600.0)
                .build()?;
            refresh_menu(&handle, None);
            // `Moviclone Studio <project folder>` opens it at once (shortcuts, scripts).
            if let Some(dir) = std::env::args_os().nth(1).map(PathBuf::from).filter(|d| projects::is_project(d)) {
                let app = handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = open(&app, dir).await {
                        show_error(&app, "The project could not be opened", &e);
                    }
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Moviclone Studio failed to start");

    app.run(|app, event| match event {
        RunEvent::WindowEvent { label, event: tauri::WindowEvent::CloseRequested { api, .. }, .. } if label == MAIN => {
            // Unsaved studio edits live in the page; ask before the window goes.
            if project_open(app) {
                api.prevent_close();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if confirm_close(&app, "Quit Moviclone Studio").await {
                        stop_runtime(&app).await;
                        app.exit(0);
                    }
                });
            }
        }
        RunEvent::Exit => {
            tauri::async_runtime::block_on(stop_runtime(app));
        }
        _ => {}
    });
}

fn init_logging(app: &AppHandle) {
    let file = app.path().app_log_dir().ok().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::OpenOptions::new().create(true).append(true).open(dir.join("moviclone.log")).ok()
    });
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tokio_modbus=warn".into());
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false);
    let _ = match file {
        Some(f) => builder.with_writer(Mutex::new(f)).try_init(),
        None => builder.try_init(),
    };
}

// ---------------------------------------------------------------- runtime

fn web_dir(app: &AppHandle) -> PathBuf {
    let bundled = app.path().resource_dir().map(|d| d.join("web"));
    match bundled {
        Ok(d) if d.join("studio").is_dir() => d,
        // `cargo run` in desktop/: the repository's web folder.
        _ => Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("web"),
    }
}

/// The bundled site template (`template/site-template.toml`), or the
/// repository's demo when running from source.
fn site_manifest(app: &AppHandle) -> PathBuf {
    let bundled = app.path().resource_dir().map(|d| d.join("template").join("site-template.toml"));
    match bundled {
        Ok(p) if p.is_file() => p,
        _ => Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("demo").join("site-template.toml"),
    }
}

fn pick_listen() -> SocketAddr {
    for port in PREFERRED_PORTS {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        if TcpListener::bind(addr).is_ok() {
            return addr;
        }
    }
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

fn project_open(app: &AppHandle) -> bool {
    app.state::<AppState>().running.try_lock().map(|r| r.is_some()).unwrap_or(true)
}

async fn stop_runtime(app: &AppHandle) {
    let state = app.state::<AppState>();
    let running = state.running.lock().await.take();
    if let Some(r) = running {
        tracing::info!(project = %r.dir.display(), "closing project");
        if let Some(w) = app.get_webview_window(HMI) {
            let _ = w.close();
        }
        match tokio::time::timeout(Duration::from_secs(15), r.rt.shutdown()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("runtime stopped with an error: {e:#}"),
            Err(_) => tracing::warn!("runtime did not stop within 15 s"),
        }
    }
}

/// Starts the runtime on `dir` (closing any open project) and shows its studio.
async fn open(app: &AppHandle, dir: PathBuf) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _switch = state.switching.lock().await;
    if !projects::is_project(&dir) {
        state.update_settings(|s| s.forget(&dir));
        return Err(format!("{} holds no Moviclone project (project.toml)", dir.display()));
    }
    stop_runtime(app).await;
    let name = projects::project_name(&dir);
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?.join("projects").join(projects::data_key(&dir));
    let simulate = state.settings().simulate;
    let mut args = moviclone_rt::Args::new(&dir);
    args.listen = pick_listen();
    args.simulate = simulate;
    args.studio = true;
    // A project with errors still opens: the engineer fixes it in the studio.
    args.allow_errors = true;
    args.web = web_dir(app);
    args.data = Some(data.clone());
    tracing::info!(project = %dir.display(), simulate, data = %data.display(), "opening project");
    let rt = moviclone_rt::start(args).await.map_err(|e| format!("{e:#}"))?;
    let addr = rt.local_addr();
    *state.running.lock().await = Some(Running { rt, dir: dir.clone(), name: name.clone(), addr, data });
    state.update_settings(|s| s.remember(&dir));

    if let Some(w) = app.get_webview_window(MAIN) {
        let url: Url = format!("http://{addr}/studio/").parse().map_err(|e| format!("{e}"))?;
        w.navigate(url).map_err(|e| e.to_string())?;
        let _ = w.set_title(&format!("{name} – Moviclone Studio{}", if simulate { " (simulator)" } else { "" }));
    }
    refresh_menu(app, Some(&name));
    Ok(())
}

/// Back to the launcher.
async fn close(app: &AppHandle) {
    stop_runtime(app).await;
    if let Some(w) = app.get_webview_window(MAIN) {
        let launcher = if cfg!(windows) { "http://tauri.localhost/index.html" } else { "tauri://localhost/index.html" };
        if let Ok(url) = launcher.parse::<Url>() {
            let _ = w.navigate(url);
        }
        let _ = w.set_title("Moviclone Studio");
    }
    refresh_menu(app, None);
}

async fn confirm_close(app: &AppHandle, what: &str) -> bool {
    let app = app.clone();
    let what = what.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .message("Unsaved studio edits are lost. Save them first (Ctrl+S, or Save All).")
            .title(what.clone())
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(what, "Cancel".into()))
            .blocking_show()
    })
    .await
    .unwrap_or(false)
}

fn show_error(app: &AppHandle, title: &str, message: &str) {
    tracing::warn!("{title}: {message}");
    app.dialog().message(message).title(title).kind(MessageDialogKind::Error).show(|_| {});
}

async fn pick_folder(app: &AppHandle, title: &str) -> Option<PathBuf> {
    let app = app.clone();
    let title = title.to_string();
    tauri::async_runtime::spawn_blocking(move || app.dialog().file().set_title(title).blocking_pick_folder())
        .await
        .ok()
        .flatten()
        .and_then(|p| p.into_path().ok())
}

/// Asks before replacing an open project. `true` to go on.
async fn may_switch(app: &AppHandle) -> bool {
    !project_open(app) || confirm_close(app, "Close the current project").await
}

// ---------------------------------------------------------------- actions (menu and launcher)

async fn action_open(app: &AppHandle) -> Result<bool, String> {
    let Some(dir) = pick_folder(app, "Open a Moviclone project folder").await else { return Ok(false) };
    if !may_switch(app).await {
        return Ok(false);
    }
    open(app, dir).await.map(|_| true)
}

async fn action_new(app: &AppHandle) -> Result<bool, String> {
    let Some(dir) = pick_folder(app, "Choose an empty folder for the new project (its name becomes the project name)").await else {
        return Ok(false);
    };
    if !may_switch(app).await {
        return Ok(false);
    }
    let d = dir.clone();
    tauri::async_runtime::spawn_blocking(move || projects::create(&d))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
    open(app, dir).await.map(|_| true)
}

async fn action_import(app: &AppHandle) -> Result<bool, String> {
    let a = app.clone();
    let source = tauri::async_runtime::spawn_blocking(move || {
        a.dialog()
            .file()
            .set_title("Movicon.NExT project to import")
            .add_filter("Movicon.NExT project", &["UFProject"])
            .blocking_pick_file()
    })
    .await
    .ok()
    .flatten()
    .and_then(|p| p.into_path().ok());
    let Some(source) = source else { return Ok(false) };
    let Some(out) = pick_folder(app, "Choose an empty folder for the converted project").await else { return Ok(false) };
    if !may_switch(app).await {
        return Ok(false);
    }
    let o = out.clone();
    let name = tauri::async_runtime::spawn_blocking(move || projects::import_movicon(&source, &o))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
    open(app, out.clone()).await?;
    app.dialog()
        .message(format!("{name} was converted. What was mapped, assumed or skipped is in IMPORT-REPORT.md in {}.", out.display()))
        .title("Movicon project imported")
        .kind(MessageDialogKind::Info)
        .show(|_| {});
    Ok(true)
}

async fn action_simulate(app: &AppHandle, on: bool) {
    let state = app.state::<AppState>();
    state.update_settings(|s| s.simulate = on);
    let dir = state.running.lock().await.as_ref().map(|r| r.dir.clone());
    refresh_menu(app, None);
    if let Some(dir) = dir {
        // The runtime picks drivers or the simulator at start: restart it.
        if confirm_close(app, if on { "Switch to the simulator" } else { "Connect to the devices" }).await {
            if let Err(e) = open(app, dir).await {
                show_error(app, "The project could not be reopened", &e);
            }
        } else {
            state.update_settings(|s| s.simulate = !on);
            refresh_menu(app, None);
        }
    }
}

fn spawn_action<F>(app: &AppHandle, title: &'static str, f: impl FnOnce(AppHandle) -> F + Send + 'static)
where
    F: std::future::Future<Output = Result<bool, String>> + Send + 'static,
{
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = f(app.clone()).await {
            show_error(&app, title, &e);
        }
    });
}

// ---------------------------------------------------------------- menu

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let item = |id: &str, text: &str, accel: Option<&str>| {
        let mut b = MenuItemBuilder::with_id(id, text);
        if let Some(a) = accel {
            b = b.accelerator(a);
        }
        b.build(app)
    };
    let mut project = SubmenuBuilder::new(app, "Project")
        .item(&item("open", "Open Project…", Some("CmdOrCtrl+O"))?)
        .item(&item("depot", "Set Up a Depot…", None)?)
        .item(&item("new", "New Blank Project…", Some("CmdOrCtrl+Shift+N"))?)
        .item(&item("import", "Import Movicon.NExT Project…", None)?)
        .separator()
        .item(&item("close", "Close Project", None)?)
        .item(&item("project_folder", "Show Project Folder", None)?);
    if !cfg!(target_os = "macos") {
        project = project.separator().quit();
    }
    let simulate = CheckMenuItemBuilder::with_id("simulate", "Use the Simulator").checked(true).build(app)?;
    let runtime = SubmenuBuilder::new(app, "Runtime")
        .item(&simulate)
        .separator()
        .item(&item("hmi_window", "Open HMI Window", Some("F6"))?)
        .item(&item("hmi_browser", "Open HMI in Browser", None)?)
        .item(&item("studio_browser", "Open Studio in Browser", None)?)
        .separator()
        .item(&item("data_folder", "Show Data Folder (history, alarms, users)", None)?)
        .item(&item("log", "Show Log File", None)?)
        .build()?;
    let help = SubmenuBuilder::new(app, "Help").item(&item("about", "About Moviclone Studio", None)?).build()?;
    let mut menu = MenuBuilder::new(app);
    if cfg!(target_os = "macos") {
        // macOS: the app menu, and an Edit menu so copy, paste and undo reach the pages.
        let app_menu = SubmenuBuilder::new(app, "Moviclone Studio")
            .item(&item("about", "About Moviclone Studio", None)?)
            .separator()
            .hide()
            .hide_others()
            .show_all()
            .separator()
            .quit()
            .build()?;
        let edit = SubmenuBuilder::new(app, "Edit").undo().redo().separator().cut().copy().paste().select_all().build()?;
        menu = menu.item(&app_menu).item(&project.build()?).item(&edit);
    } else {
        menu = menu.item(&project.build()?);
    }
    menu.item(&runtime).item(&help).build()
}

/// Enables what makes sense with or without an open project.
fn refresh_menu(app: &AppHandle, _name: Option<&str>) {
    let Some(menu) = app.menu() else { return };
    let open = project_open(app);
    for id in ["close", "project_folder", "hmi_window", "hmi_browser", "studio_browser", "data_folder"] {
        if let Some(item) = menu.get(id).and_then(|k| k.as_menuitem().cloned()) {
            let _ = item.set_enabled(open);
        }
    }
    let simulate = app.state::<AppState>().settings().simulate;
    if let Some(item) = menu.get("simulate").and_then(|k| k.as_check_menuitem().cloned()) {
        let _ = item.set_checked(simulate);
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => spawn_action(app, "The project could not be opened", |a| async move { action_open(&a).await }),
        "new" => spawn_action(app, "The project could not be created", |a| async move { action_new(&a).await }),
        "import" => spawn_action(app, "The Movicon project could not be imported", |a| async move { action_import(&a).await }),
        "depot" => {
            // The launcher holds the setup; leave the open project first.
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if may_switch(&app).await {
                    close(&app).await;
                    if let Some(w) = app.get_webview_window(MAIN) {
                        let _ = w.eval("setTimeout(() => window.showSetup && window.showSetup(), 400)");
                    }
                }
            });
        }
        "close" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if confirm_close(&app, "Close Project").await {
                    close(&app).await;
                }
            });
        }
        "simulate" => {
            let app = app.clone();
            // The check mark has already flipped; read the wish from it.
            let on = app.menu().and_then(|m| m.get("simulate")).and_then(|k| k.as_check_menuitem().cloned()).and_then(|i| i.is_checked().ok()).unwrap_or(true);
            tauri::async_runtime::spawn(async move { action_simulate(&app, on).await });
        }
        "hmi_window" | "hmi_browser" | "studio_browser" | "project_folder" | "data_folder" => {
            let app = app.clone();
            let id = id.to_string();
            tauri::async_runtime::spawn(async move { open_place(&app, &id).await });
        }
        "log" => {
            if let Ok(dir) = app.path().app_log_dir() {
                let _ = app.opener().reveal_item_in_dir(dir.join("moviclone.log"));
            }
        }
        "about" => {
            let version = app.package_info().version.to_string();
            app.dialog()
                .message(format!(
                    "Moviclone Studio {version}\n\nThe studio, the operator HMI and the runtime (drivers, simulator, historian, alarms) in one app. All artwork is original."
                ))
                .title("About Moviclone Studio")
                .show(|_| {});
        }
        _ => {}
    }
}

async fn open_place(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    let (addr, name, dir, data) = {
        let guard = state.running.lock().await;
        let Some(r) = guard.as_ref() else { return };
        (r.addr, r.name.clone(), r.dir.clone(), r.data.clone())
    };
    let result = match id {
        "hmi_window" => {
            let url: Url = match format!("http://{addr}/hmi/").parse() {
                Ok(u) => u,
                Err(_) => return,
            };
            match app.get_webview_window(HMI) {
                Some(w) => w.navigate(url).and_then(|_| w.set_focus()).map_err(|e| e.to_string()),
                None => WebviewWindowBuilder::new(app, HMI, WebviewUrl::External(url))
                    .title(format!("{name} – HMI"))
                    .inner_size(1600.0, 900.0)
                    .build()
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
            }
        }
        "hmi_browser" => app.opener().open_url(format!("http://{addr}/hmi/"), None::<&str>).map_err(|e| e.to_string()),
        "studio_browser" => app.opener().open_url(format!("http://{addr}/studio/"), None::<&str>).map_err(|e| e.to_string()),
        "project_folder" => app.opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(|e| e.to_string()),
        "data_folder" => app.opener().open_path(data.to_string_lossy(), None::<&str>).map_err(|e| e.to_string()),
        _ => Ok(()),
    };
    if let Err(e) = result {
        show_error(app, "Could not open it", &e);
    }
}

// ---------------------------------------------------------------- launcher commands

#[derive(Serialize)]
struct RecentProject {
    path: String,
    name: String,
    exists: bool,
}

#[derive(Serialize)]
struct LauncherState {
    version: String,
    simulate: bool,
    /// First start: show the depot setup.
    setup: bool,
    recent: Vec<RecentProject>,
}

#[tauri::command]
fn launcher_state(app: AppHandle) -> LauncherState {
    let s = app.state::<AppState>().settings();
    LauncherState {
        version: app.package_info().version.to_string(),
        simulate: s.simulate,
        setup: !s.setup_done && s.recent.is_empty(),
        recent: s
            .recent
            .iter()
            .map(|p| RecentProject { path: p.to_string_lossy().into_owned(), name: projects::project_name(p), exists: projects::is_project(p) })
            .collect(),
    }
}

#[tauri::command]
async fn open_project_dialog(app: AppHandle) -> Result<bool, String> {
    action_open(&app).await
}

#[tauri::command]
async fn open_recent(app: AppHandle, path: String) -> Result<bool, String> {
    open(&app, PathBuf::from(path)).await.map(|_| true)
}

#[tauri::command]
fn forget_recent(app: AppHandle, path: String) {
    app.state::<AppState>().update_settings(|s| s.forget(Path::new(&path)));
}

#[tauri::command]
async fn new_project(app: AppHandle) -> Result<bool, String> {
    action_new(&app).await
}

#[tauri::command]
async fn import_movicon(app: AppHandle) -> Result<bool, String> {
    action_import(&app).await
}

#[tauri::command]
fn set_simulate(app: AppHandle, on: bool) {
    app.state::<AppState>().update_settings(|s| s.simulate = on);
    refresh_menu(&app, None);
}

// ---------------------------------------------------------------- depot setup (first start)

#[derive(Serialize)]
struct TemplateMeter {
    key: String,
    product: String,
    color: String,
    tag_prefix: String,
}

#[derive(Serialize)]
struct SiteTemplate {
    meters: Vec<TemplateMeter>,
    /// Where a depot's project goes unless the user picks another folder.
    projects_dir: String,
}

#[tauri::command]
fn site_template(app: AppHandle) -> Result<SiteTemplate, String> {
    let manifest = moviclone_site::Manifest::load(&site_manifest(&app)).map_err(|e| format!("{e:#}"))?;
    let docs = app.path().document_dir().or_else(|_| app.path().home_dir()).map_err(|e| e.to_string())?;
    Ok(SiteTemplate {
        meters: manifest
            .meters
            .into_iter()
            .map(|m| TemplateMeter { key: m.key, product: m.product, color: m.color, tag_prefix: m.tag_prefix })
            .collect(),
        projects_dir: docs.join("Moviclone").to_string_lossy().into_owned(),
    })
}

#[tauri::command]
async fn choose_folder(app: AppHandle) -> Option<String> {
    pick_folder(&app, "Where to keep the depot's project").await.map(|p| p.to_string_lossy().into_owned())
}

/// Builds the depot's SCADA with the chosen meters into `parent/<depot>` and opens it.
#[tauri::command]
async fn create_depot(app: AppHandle, depot: String, meters: Vec<String>, parent: String) -> Result<bool, String> {
    if depot.trim().is_empty() {
        return Err("Enter the depot name.".into());
    }
    if meters.is_empty() {
        return Err("Choose at least one meter.".into());
    }
    let out = PathBuf::from(&parent).join(projects::folder_name(&depot));
    if projects::is_project(&out) {
        return Err(format!("{} already holds a project. Open it from Open Project, or choose another folder.", out.display()));
    }
    if !may_switch(&app).await {
        return Ok(false);
    }
    let manifest = site_manifest(&app);
    let o = out.clone();
    tauri::async_runtime::spawn_blocking(move || projects::create_depot(&manifest, &depot, &meters, &o))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
    app.state::<AppState>().update_settings(|s| s.setup_done = true);
    tracing::info!(project = %out.display(), "depot project built");
    open(&app, out).await.map(|_| true)
}

#[tauri::command]
fn skip_setup(app: AppHandle) {
    app.state::<AppState>().update_settings(|s| s.setup_done = true);
}

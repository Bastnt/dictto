//! The settings window: the Windows pages General, Audio, API, Speech and Shortcuts, each
//! control saving through the daemon. Unsaved edits are dropped with the window, and the
//! process exits when it closes.
mod api;
mod audio;
mod general;
mod shortcuts;
mod speech;

use crate::bus::{Client, Incoming};
use crate::form::{self, Backend, Catalog, Settings};
use adw::prelude::*;
use dictto_core::events;
use gtk::{gio, glib};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

const APP_ID: &str = "com.dictto.Settings";
/// The daemon owns its bus name before its runtime exists and answers this meanwhile.
const STARTING: &str = "Dictto is still starting";
const PREVIEW: &str = "Preview: recording services disabled";
const NOT_RUNNING: &str = "Dictto is not running";
const PAGES: [&str; 5] = ["general", "audio", "api", "speech", "shortcuts"];

pub(crate) fn run() -> glib::ExitCode {
    let mut args = std::env::args().skip(1);
    let mut screenshots = None;
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            // Development aid: saves each page as PNG and quits.
            ("--screenshot", Some(dir)) => screenshots = Some(PathBuf::from(dir)),
            _ => {
                eprintln!("dictto-gnome-settings: unknown argument: {arg}");
                return glib::ExitCode::from(2);
            }
        }
    }
    let flags = if screenshots.is_some() {
        gio::ApplicationFlags::NON_UNIQUE
    } else {
        gio::ApplicationFlags::empty()
    };
    // The application id makes a second start present the existing window instead.
    let app = adw::Application::builder().application_id(APP_ID).flags(flags).build();
    app.connect_activate(move |app| match app.active_window() {
        Some(window) => window.present(),
        None => Ui::build(app, screenshots.clone()),
    });
    app.run_with_args::<&str>(&[])
}

pub(super) struct Ui {
    app: adw::Application,
    window: adw::ApplicationWindow,
    root: gtk::Stack,
    stack: adw::ViewStack,
    switcher: adw::ViewSwitcher,
    absent: adw::StatusPage,
    start: gtk::Button,
    status: gtk::Label,
    client: RefCell<Option<Client>>,
    /// Last values read from or saved through the daemon.
    settings: RefCell<Settings>,
    /// Unsaved per-model Speech texts, kept while switching models.
    drafts: RefCell<Settings>,
    catalog: RefCell<Catalog>,
    /// `microphone_device` value of each microphone list entry.
    microphones: RefCell<Vec<String>>,
    preview: Cell<bool>,
    field_focus: Cell<bool>,
    /// Set while widgets are filled from settings, so their change handlers stay quiet.
    updating: Cell<bool>,
    first_presence: Cell<bool>,
    screenshots: Option<PathBuf>,
    general: general::Page,
    audio: audio::Page,
    api: api::Page,
    speech: speech::Page,
    shortcuts: shortcuts::Page,
}

impl Ui {
    fn build(app: &adw::Application, screenshots: Option<PathBuf>) {
        let (general, audio, api, speech, shortcuts) =
            (general::Page::new(), audio::Page::new(), api::Page::new(), speech::Page::new(), shortcuts::Page::new());
        let stack = adw::ViewStack::new();
        let pages: [(&adw::PreferencesPage, &str, &str); 5] = [
            (&general.page, "General", "preferences-system-symbolic"),
            (&audio.page, "Audio", "audio-input-microphone-symbolic"),
            (&api.page, "API", "network-server-symbolic"),
            (&speech.page, "Speech", "audio-speakers-symbolic"),
            (&shortcuts.page, "Shortcuts", "preferences-desktop-keyboard-shortcuts-symbolic"),
        ];
        for ((page, title, icon), name) in pages.into_iter().zip(PAGES) {
            stack.add_titled_with_icon(page, Some(name), title, icon);
        }
        let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();
        switcher.set_visible(false);
        let header = adw::HeaderBar::builder().title_widget(&switcher).build();
        let status = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        let start = gtk::Button::builder()
            .label("Start Dictto")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let absent = adw::StatusPage::builder()
            .icon_name("dialog-information-symbolic")
            .title(NOT_RUNNING)
            .description(START_HINT)
            .child(&start)
            .build();
        let root = gtk::Stack::new();
        root.add_named(&gtk::Box::new(gtk::Orientation::Vertical, 0), Some("connecting"));
        root.add_named(&absent, Some("absent"));
        root.add_named(&stack, Some("settings"));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&root));
        toolbar.add_bottom_bar(&status);
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Dictto Settings")
            .default_width(820)
            .default_height(700)
            .content(&toolbar)
            .build();
        let ui = Rc::new(Ui {
            app: app.clone(),
            window,
            root,
            stack,
            switcher,
            absent,
            start,
            status,
            client: RefCell::new(None),
            settings: RefCell::default(),
            drafts: RefCell::default(),
            catalog: RefCell::default(),
            microphones: RefCell::default(),
            preview: Cell::new(false),
            field_focus: Cell::new(false),
            updating: Cell::new(false),
            first_presence: Cell::new(true),
            screenshots,
            general,
            audio,
            api,
            speech,
            shortcuts,
        });
        general::connect(&ui);
        audio::connect(&ui);
        api::connect(&ui);
        speech::connect(&ui);
        shortcuts::connect(&ui);
        ui.connect_window();
        ui.window.present();
        spawn(ui.clone().follow_daemon());
    }

    fn connect_window(self: &Rc<Self>) {
        let ui = self.clone();
        self.window.connect_is_active_notify(move |_| ui.update_capture());
        let ui = self.clone();
        self.window.connect_close_request(move |_| {
            // The daemon also drops capture when this process leaves the bus.
            if let Some(client) = ui.client() {
                client.release_capture();
            }
            glib::Propagation::Proceed
        });
        let ui = self.clone();
        self.stack.connect_visible_child_name_notify(move |stack| {
            if stack.visible_child_name().as_deref() == Some("speech") {
                speech::shown(&ui);
            }
        });
        let ui = self.clone();
        self.start.connect_clicked(move |_| ui.start_daemon());
    }

    fn client(&self) -> Option<Client> {
        self.client.borrow().clone()
    }

    fn set_status(&self, text: &str) {
        self.status.set_text(text);
    }

    /// Shows "Saved" or the error, like the Windows status line; true on success.
    fn report(&self, result: Result<(), String>) -> bool {
        match result {
            Ok(()) => {
                self.set_status(form::SAVED);
                true
            }
            Err(error) => {
                self.set_status(&error);
                false
            }
        }
    }

    async fn save(&self, key: &str, value: &str) -> bool {
        self.report(form::persist(self, key, value).await)
    }

    fn quietly(&self, fill: impl FnOnce()) {
        let previous = self.updating.replace(true);
        fill();
        self.updating.set(previous);
    }

    fn set_field_focus(&self, focused: bool) {
        self.field_focus.set(focused);
        self.update_capture();
    }

    /// Capture follows the focused shortcut field, and stops while the window is in the
    /// background so the dictation shortcut keeps working in other apps.
    fn update_capture(&self) {
        if let Some(client) = self.client() {
            client.set_capture(self.field_focus.get() && self.window.is_active());
        }
    }

    async fn follow_daemon(self: Rc<Self>) {
        let (client, mut incoming) = match Client::connect().await {
            Ok(connected) => connected,
            Err(error) => {
                self.show_absent();
                self.start.set_sensitive(false);
                self.absent.set_description(Some(&format!("Cannot reach the session bus: {error}")));
                return;
            }
        };
        *self.client.borrow_mut() = Some(client);
        while let Some(message) = incoming.recv().await {
            match message {
                Incoming::Present(true) => spawn(self.clone().load()),
                Incoming::Present(false) => {
                    self.show_absent();
                    if self.first_presence.replace(false) {
                        if let Some(dir) = self.screenshots.clone() {
                            spawn(self.clone().screenshot(dir, vec!["not-running".into()]));
                        }
                    }
                }
                Incoming::Event(name, payload) => self.event(&name, &payload),
            }
        }
    }

    fn show_absent(&self) {
        self.root.set_visible_child_name("absent");
        self.switcher.set_visible(false);
        self.start.set_sensitive(true);
        self.absent.set_description(Some(START_HINT));
        // A restarted daemon starts without capture; `load` asks again if still needed.
        if let Some(client) = self.client() {
            client.set_capture(false);
        }
    }

    async fn load(self: Rc<Self>) {
        self.first_presence.set(false);
        let Some(client) = self.client() else { return };
        let mut settings = client.get_settings().await;
        for _ in 0..50 {
            if !matches!(&settings, Err(error) if error == STARTING) {
                break;
            }
            glib::timeout_future(Duration::from_millis(200)).await;
            settings = client.get_settings().await;
        }
        match settings {
            Ok(settings) => *self.settings.borrow_mut() = settings,
            Err(error) => self.set_status(&error),
        }
        self.drafts.borrow_mut().clear();
        self.catalog.borrow_mut().invalidate();
        let status = client.status().await.unwrap_or_default();
        self.preview.set(status == PREVIEW);
        if !status.is_empty() && status != "ok" {
            self.set_status(&status);
        }
        general::fill(&self);
        audio::fill(&self);
        api::fill(&self);
        speech::render(&self);
        shortcuts::fill(&self);
        api::refresh_hint(&self).await;
        audio::refresh_microphones(&self).await;
        self.root.set_visible_child_name("settings");
        self.switcher.set_visible(true);
        self.update_capture();
        if self.stack.visible_child_name().as_deref() == Some("speech") {
            speech::shown(&self);
        }
        if let Some(dir) = self.screenshots.clone() {
            self.screenshot(dir, PAGES.iter().map(|p| p.to_string()).collect()).await;
        }
    }

    fn event(self: &Rc<Self>, name: &str, payload: &str) {
        let payload: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
        match name {
            events::SPEECH_CATALOG => speech::receive(self, &payload),
            events::AUDIO_DEVICES_CHANGED => {
                let ui = self.clone();
                spawn(async move { audio::refresh_microphones(&ui).await });
            }
            events::SETTING_CHANGED => {
                if let (Some(key), Some(value)) = (payload["key"].as_str(), payload["value"].as_str()) {
                    self.settings.borrow_mut().insert(key.into(), value.into());
                }
            }
            _ => {
                if let Some(text) = form::event_status(name, &payload) {
                    self.set_status(&text);
                }
            }
        }
    }

    fn start_daemon(self: &Rc<Self>) {
        if let Err(error) = launch_daemon() {
            self.absent.set_description(Some(&error));
            return;
        }
        self.start.set_sensitive(false);
        self.absent.set_description(Some("Starting Dictto…"));
        let ui = self.clone();
        spawn(async move {
            glib::timeout_future(Duration::from_secs(15)).await;
            if ui.root.visible_child_name().as_deref() == Some("absent") {
                ui.start.set_sensitive(true);
                ui.absent.set_description(Some("Dictto did not start. Its log is in the user journal."));
            }
        });
    }

    /// Saves the listed pages ("not-running" is the page shown without a daemon) as PNG
    /// files in `dir`, then quits.
    async fn screenshot(self: Rc<Self>, dir: PathBuf, pages: Vec<String>) {
        if let Err(error) = std::fs::create_dir_all(&dir) {
            log::error!("cannot create {}: {error}", dir.display());
        }
        for (index, name) in pages.iter().enumerate() {
            if PAGES.contains(&name.as_str()) {
                self.stack.set_visible_child_name(name);
            }
            glib::timeout_future(Duration::from_millis(1500)).await;
            let path = dir.join(format!("{}-{name}.png", index + 1));
            match save_png(&self.window, &path) {
                Ok(()) => log::info!("saved {}", path.display()),
                Err(error) => log::error!("cannot save {}: {error}", path.display()),
            }
        }
        self.app.quit();
    }
}

const START_HINT: &str = "Settings are kept by Dictto. Start it to change them.";

impl Backend for Ui {
    async fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let client = self.client().ok_or(NOT_RUNNING)?;
        client.set_setting(key, value).await?;
        self.settings.borrow_mut().insert(key.into(), value.into());
        Ok(())
    }

    async fn save_key(&self, key: &str) -> Result<(), String> {
        self.client().ok_or(NOT_RUNNING)?.save_key(key).await
    }

    async fn remove_key(&self) -> Result<(), String> {
        self.client().ok_or(NOT_RUNNING)?.remove_key().await
    }
}

fn spawn(future: impl Future<Output = ()> + 'static) {
    glib::spawn_future_local(future);
}

/// Saves a setting from a switch, putting the switch back when the daemon refuses.
fn toggle(ui: &Rc<Ui>, row: &adw::SwitchRow, key: &'static str, after: impl Fn(&Ui) + 'static) {
    let ui = ui.clone();
    let after = Rc::new(after);
    row.connect_active_notify(move |row| {
        if ui.updating.get() {
            return;
        }
        let (ui, row, after) = (ui.clone(), row.clone(), after.clone());
        let on = row.is_active();
        spawn(async move {
            if !ui.save(key, form::bool_value(on)).await {
                ui.quietly(|| row.set_active(form::is_on(&ui.settings.borrow(), key)));
            }
            after(&ui);
        });
    });
}

fn button(label: &str) -> gtk::Button {
    gtk::Button::builder().label(label).valign(gtk::Align::Center).build()
}

fn entry(width: i32) -> gtk::Entry {
    gtk::Entry::builder().width_chars(width).valign(gtk::Align::Center).build()
}

fn string_list(labels: &[String]) -> gtk::StringList {
    let items: Vec<&str> = labels.iter().map(String::as_str).collect();
    gtk::StringList::new(&items)
}

/// The daemon binary sits next to this one; started detached, in daemon mode.
fn launch_daemon() -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()
        .map_err(|e| format!("Cannot locate Dictto: {e}"))?
        .with_file_name("dictto-gnome");
    let mut child = std::process::Command::new(&exe)
        .stdin(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("Cannot start {}: {e}", exe.display()))?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

fn save_png(window: &adw::ApplicationWindow, path: &Path) -> Result<(), String> {
    let (width, height) = (window.width(), window.height());
    let snapshot = gtk::Snapshot::new();
    gtk::WidgetPaintable::new(Some(window)).snapshot(&snapshot, width as f64, height as f64);
    let node = snapshot.to_node().ok_or("the window drew nothing")?;
    let renderer = window.renderer().ok_or("the window has no renderer")?;
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    renderer.render_texture(&node, Some(&bounds)).save_to_png(path).map_err(|e| e.to_string())
}

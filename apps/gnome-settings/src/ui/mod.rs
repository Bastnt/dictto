//! The settings window: the Windows tabs General, Dictation, Voice, Shortcuts and
//! Connection. Every control saves through the daemon as it changes; typed text saves when
//! Enter is pressed or focus leaves, and on close. The process exits when the window closes.
mod connection;
mod dictation;
mod general;
mod shortcuts;
mod voice;

use crate::bus::{Client, Incoming};
use crate::form::{self, Backend, Catalog, Choices, Models, Settings};
use adw::prelude::*;
use dictto_core::events;
use gtk::{gio, glib};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

const APP_ID: &str = "com.dictto.Settings";
/// The daemon owns its bus name before its runtime exists and answers this meanwhile.
const STARTING: &str = "Dictto is still starting";
const PREVIEW: &str = "Preview: recording services disabled";
const NOT_RUNNING: &str = "Dictto is not running";
const PAGES: [&str; 5] = ["general", "dictation", "voice", "shortcuts", "connection"];

/// A save of typed text still in its field, run before the window closes.
type Pending = Option<Pin<Box<dyn Future<Output = ()>>>>;

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
    /// Unsaved per-model Voice texts, kept while switching models.
    drafts: RefCell<Settings>,
    catalog: RefCell<Catalog>,
    models: RefCell<Models>,
    microphones: RefCell<Vec<form::Microphone>>,
    has_key: Cell<bool>,
    preview: Cell<bool>,
    field_focus: Cell<bool>,
    /// Set while widgets are filled from settings, so their change handlers stay quiet.
    updating: Cell<bool>,
    first_presence: Cell<bool>,
    /// Counts daemon arrivals and departures; a `load` started for an earlier one stops.
    presence: Cell<u64>,
    /// Typed values are being saved before the window closes.
    closing: Cell<bool>,
    screenshots: Option<PathBuf>,
    general: general::Page,
    dictation: dictation::Page,
    voice: voice::Page,
    shortcuts: shortcuts::Page,
    connection: connection::Page,
}

impl Ui {
    fn build(app: &adw::Application, screenshots: Option<PathBuf>) {
        let (general, dictation, voice, shortcuts, connection) = (
            general::Page::new(),
            dictation::Page::new(),
            voice::Page::new(),
            shortcuts::Page::new(),
            connection::Page::new(),
        );
        let stack = adw::ViewStack::new();
        let pages: [(&adw::PreferencesPage, &str, &str); 5] = [
            (&general.page, "General", "preferences-system-symbolic"),
            (&dictation.page, "Dictation", "audio-input-microphone-symbolic"),
            (&voice.page, "Voice", "audio-speakers-symbolic"),
            (&shortcuts.page, "Shortcuts", "preferences-desktop-keyboard-shortcuts-symbolic"),
            (&connection.page, "Connection", "network-server-symbolic"),
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
            .default_height(640)
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
            models: RefCell::default(),
            microphones: RefCell::default(),
            has_key: Cell::new(false),
            preview: Cell::new(false),
            field_focus: Cell::new(false),
            updating: Cell::new(false),
            first_presence: Cell::new(true),
            presence: Cell::new(0),
            closing: Cell::new(false),
            screenshots,
            general,
            dictation,
            voice,
            shortcuts,
            connection,
        });
        general::connect(&ui);
        dictation::connect(&ui);
        voice::connect(&ui);
        shortcuts::connect(&ui);
        connection::connect(&ui);
        ui.connect_window();
        ui.window.present();
        spawn(ui.clone().follow_daemon());
    }

    fn connect_window(self: &Rc<Self>) {
        let ui = self.clone();
        self.window.connect_is_active_notify(move |_| ui.update_capture());
        let ui = self.clone();
        self.window.connect_close_request(move |window| {
            if ui.closing.replace(true) {
                // The daemon also drops capture when this process leaves the bus.
                if let Some(client) = ui.client() {
                    client.release_capture();
                }
                return glib::Propagation::Proceed;
            }
            // Focus does not leave a field when the window closes, so its text saves here.
            let saves = [connection::pending(&ui), voice::pending(&ui)];
            let window = window.clone();
            spawn(async move {
                for save in saves.into_iter().flatten() {
                    save.await;
                }
                window.close();
            });
            glib::Propagation::Stop
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

    fn set_has_key(&self, has_key: bool) {
        self.has_key.set(has_key);
        // Windows marks the tab "Connection (!)".
        self.stack.page(&self.connection.page).set_needs_attention(!has_key);
    }

    /// Asks the endpoint for its models once per endpoint and key; the lists then stay
    /// until Refresh. A preview daemon reaches no endpoint, as on Windows.
    fn autoload(self: &Rc<Self>) {
        if self.preview.get() || !self.has_key.get() {
            return;
        }
        let idle = {
            let models = self.models.borrow();
            !models.loaded && !models.loading
        };
        if idle {
            self.load_models(false);
        }
        let empty = {
            let catalog = self.catalog.borrow();
            catalog.models.is_empty() && !catalog.loading
        };
        if empty {
            voice::refresh(self);
        }
    }

    fn load_models(self: &Rc<Self>, refresh: bool) {
        let generation = self.models.borrow_mut().begin();
        self.show_models();
        let ui = self.clone();
        spawn(async move {
            let Some(client) = ui.client() else { return };
            if let Err(error) = client.discover_models(generation, refresh).await {
                ui.receive_models(&serde_json::json!({"generation": generation, "error": error}));
            }
        });
    }

    fn receive_models(&self, payload: &Value) {
        if !self.models.borrow_mut().receive(payload) {
            return;
        }
        let status = match payload["error"].as_str() {
            Some(error) => format!("Models: {error}"),
            None => self.models.borrow().status(self.has_key.get()),
        };
        self.set_status(&status);
        self.show_models();
    }

    fn show_models(&self) {
        dictation::fill(self);
        connection::show_status(self);
    }

    /// Lists that belong to the previous endpoint or key.
    fn forget_catalogs(self: &Rc<Self>) {
        self.models.borrow_mut().forget();
        self.catalog.borrow_mut().invalidate();
        self.show_models();
        voice::render(self);
        self.autoload();
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
            if let Incoming::Present(_) = message {
                self.presence.set(self.presence.get() + 1);
            }
            match message {
                Incoming::Present(true) => spawn(self.clone().load(self.presence.get())),
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

    async fn load(self: Rc<Self>, presence: u64) {
        let first = self.first_presence.replace(false);
        let Some(client) = self.client() else { return };
        let current = || self.presence.get() == presence;
        let mut settings = client.get_settings().await;
        for _ in 0..50 {
            if !current() || !matches!(&settings, Err(error) if error == STARTING) {
                break;
            }
            glib::timeout_future(Duration::from_millis(200)).await;
            settings = client.get_settings().await;
        }
        if !current() {
            return;
        }
        match settings {
            Ok(settings) => *self.settings.borrow_mut() = settings,
            // Pages filled from an empty cache would offer defaults as the saved values.
            Err(error) => {
                self.show_absent();
                self.start.set_sensitive(false);
                self.absent.set_description(Some(&error));
                return;
            }
        }
        self.drafts.borrow_mut().clear();
        self.models.borrow_mut().forget();
        self.catalog.borrow_mut().invalidate();
        let status = client.status().await.unwrap_or_default();
        if !current() {
            return;
        }
        self.preview.set(status == PREVIEW);
        if !status.is_empty() && status != "ok" {
            self.set_status(&status);
        }
        connection::refresh_hint(&self).await;
        general::refresh_microphones(&self).await;
        if !current() {
            return;
        }
        general::fill(&self);
        dictation::fill(&self);
        voice::render(&self);
        shortcuts::fill(&self);
        connection::fill(&self);
        if first {
            self.stack.set_visible_child_name(if self.has_key.get() { "general" } else { "connection" });
        }
        self.root.set_visible_child_name("settings");
        self.switcher.set_visible(true);
        self.update_capture();
        self.autoload();
        if let Some(dir) = self.screenshots.clone() {
            self.screenshot(dir, PAGES.iter().map(|p| p.to_string()).collect()).await;
        }
    }

    fn event(self: &Rc<Self>, name: &str, payload: &str) {
        let payload: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
        match name {
            "app-quitting" => self.window.close(),
            events::SPEECH_CATALOG => voice::receive(self, &payload),
            events::MODEL_CATALOG => self.receive_models(&payload),
            events::AUDIO_DEVICES_CHANGED => {
                let ui = self.clone();
                spawn(async move { general::refresh_microphones(&ui).await });
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

/// A list whose entries are stored values of one setting.
pub(super) struct List {
    pub row: adw::ComboRow,
    values: RefCell<Vec<String>>,
}

impl List {
    fn new(title: &str, subtitle: Option<&str>) -> Self {
        let row = adw::ComboRow::builder().title(title).build();
        if let Some(subtitle) = subtitle {
            row.set_subtitle(subtitle);
        }
        Self { row, values: RefCell::default() }
    }

    fn fill(&self, ui: &Ui, choices: Choices) {
        ui.quietly(|| {
            self.row.set_model(Some(&string_list(&choices.labels)));
            self.row.set_selected(choices.selected as u32);
        });
        *self.values.borrow_mut() = choices.values;
    }

    /// Picking an entry saves it; when the daemon refuses, `refill` puts the list back.
    fn bind(ui: &Rc<Ui>, list: fn(&Ui) -> &List, key: &'static str, refill: fn(&Ui)) {
        let owner = ui.clone();
        list(ui).row.connect_selected_notify(move |row| {
            if owner.updating.get() {
                return;
            }
            let Some(value) = list(&owner).values.borrow().get(row.selected() as usize).cloned() else { return };
            if owner.settings.borrow().get(key) == Some(&value) {
                return;
            }
            let ui = owner.clone();
            spawn(async move {
                if !ui.save(key, &value).await {
                    refill(&ui);
                }
            });
        });
    }
}

fn button(label: &str) -> gtk::Button {
    gtk::Button::builder().label(label).valign(gtk::Align::Center).build()
}

fn string_list(labels: &[String]) -> gtk::StringList {
    let items: Vec<&str> = labels.iter().map(String::as_str).collect();
    gtk::StringList::new(&items)
}

/// Runs `commit` when Enter is pressed in `widget` or focus leaves it, unless the page is
/// being filled.
fn on_commit(ui: &Rc<Ui>, widget: &impl IsA<gtk::Widget>, commit: impl Fn(&Rc<Ui>) + 'static) {
    let commit = Rc::new(commit);
    let focus = gtk::EventControllerFocus::new();
    {
        let (ui, commit) = (ui.clone(), commit.clone());
        focus.connect_leave(move |_| {
            if !ui.updating.get() && !ui.closing.get() {
                commit(&ui);
            }
        });
    }
    widget.add_controller(focus);
    let ui = ui.clone();
    let activate = move || {
        if !ui.updating.get() {
            commit(&ui);
        }
    };
    if let Some(entry) = widget.dynamic_cast_ref::<gtk::Entry>() {
        entry.connect_activate(move |_| activate());
    } else if let Some(row) = widget.dynamic_cast_ref::<adw::EntryRow>() {
        row.connect_entry_activated(move |_| activate());
    }
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

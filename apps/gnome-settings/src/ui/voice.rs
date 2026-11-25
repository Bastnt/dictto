//! Voice: the speech model and voice discovered from the API, voice instruction, one
//! control per engine option, the virtual microphone and the audio folder.
use super::{button, on_commit, spawn, string_list, toggle, Pending, Ui};
use crate::form;
use adw::prelude::*;
use gtk::gio;
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;

enum Control {
    Entry(gtk::Entry),
    Choice(adw::ComboRow),
}

struct EngineRow {
    name: String,
    kind: String,
    row: gtk::Widget,
    control: Control,
}

impl EngineRow {
    fn text(&self) -> String {
        match &self.control {
            Control::Entry(entry) => entry.text().trim().to_string(),
            Control::Choice(row) => form::boolean_at(row.selected()).into(),
        }
    }
}

pub(super) struct Page {
    pub page: adw::PreferencesPage,
    model: adw::ComboRow,
    refresh: gtk::Button,
    voice: adw::ComboRow,
    instruction: gtk::Entry,
    instruction_row: adw::ActionRow,
    engine_group: adw::PreferencesGroup,
    engine: RefCell<Vec<EngineRow>>,
    virtual_mic: adw::SwitchRow,
    open_folder: gtk::Button,
}

impl Page {
    pub fn new() -> Self {
        let refresh = button("Refresh");
        let model = adw::ComboRow::builder().title("Speech model").build();
        model.add_suffix(&refresh);
        let voice = adw::ComboRow::builder().title("Voice").build();
        let instruction = gtk::Entry::builder()
            .placeholder_text("Optional, for example: calm, slow, warm")
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        let instruction_row = adw::ActionRow::builder().title("Voice instruction").build();
        instruction_row.add_suffix(&instruction);
        let voices = adw::PreferencesGroup::new();
        voices.add(&model);
        voices.add(&voice);
        voices.add(&instruction_row);
        let engine_group = adw::PreferencesGroup::builder().title("Engine options").build();
        // Windows switches its communications microphone to VB-CABLE; the GNOME daemon
        // makes its own PipeWire source the default input instead.
        let virtual_mic = adw::SwitchRow::builder()
            .title("Send speech to Dictto Microphone")
            .subtitle("Switches the default microphone to Dictto Microphone while speaking, so calls hear the voice.")
            .build();
        let open_folder = button("Open audio folder");
        let routing = adw::PreferencesGroup::builder().header_suffix(&open_folder).build();
        routing.add(&virtual_mic);
        let page = adw::PreferencesPage::new();
        page.add(&voices);
        page.add(&engine_group);
        page.add(&routing);
        Self {
            page,
            model,
            refresh,
            voice,
            instruction,
            instruction_row,
            engine_group,
            engine: RefCell::default(),
            virtual_mic,
            open_folder,
        }
    }
}

pub(super) fn render(ui: &Rc<Ui>) {
    let page = &ui.voice;
    let view = form::speech_view(&ui.catalog.borrow(), &ui.settings.borrow(), &ui.drafts.borrow(), ui.has_key.get());
    ui.quietly(|| {
        page.model.set_model(Some(&string_list(&view.model_labels)));
        page.model.set_selected(view.model_index as u32);
        page.voice.set_model(Some(&string_list(&view.voice_labels)));
        page.voice.set_selected(view.voice_index as u32);
        page.instruction.set_text(&view.instruction);
        page.virtual_mic.set_active(view.virtual_mic);
        let old = std::mem::take(&mut *page.engine.borrow_mut());
        for row in old {
            page.engine_group.remove(&row.row);
        }
        let rows = view.engine.iter().map(|field| engine_row(ui, field)).collect::<Vec<_>>();
        for row in &rows {
            page.engine_group.add(&row.row);
        }
        *page.engine.borrow_mut() = rows;
    });
    page.engine_group.set_description(view.engine.is_empty().then_some("This model has no options."));
    page.model.set_sensitive(view.model_enabled);
    page.refresh.set_label(view.refresh_label);
    page.refresh.set_sensitive(view.refresh_enabled);
    page.voice.set_sensitive(view.voice_enabled);
    page.instruction_row.set_sensitive(view.instruction_enabled);
}

fn engine_row(ui: &Rc<Ui>, field: &form::EngineField) -> EngineRow {
    let (row, control) = if field.kind == "boolean" {
        let labels: Vec<String> = form::BOOLEAN_LABELS.iter().map(|l| l.to_string()).collect();
        let row = adw::ComboRow::builder().title(&field.name).model(&string_list(&labels)).build();
        row.set_selected(form::boolean_index(&field.value));
        let owner = ui.clone();
        row.connect_selected_notify(move |_| {
            if !owner.updating.get() {
                commit_engine(&owner);
            }
        });
        (row.clone().upcast::<gtk::Widget>(), Control::Choice(row))
    } else {
        let entry = gtk::Entry::builder()
            .text(&field.value)
            .placeholder_text("Default")
            .width_chars(10)
            .valign(gtk::Align::Center)
            .build();
        on_commit(ui, &entry, commit_engine);
        let row = adw::ActionRow::builder().title(&field.name).subtitle(&field.kind).build();
        row.add_suffix(&entry);
        (row.upcast::<gtk::Widget>(), Control::Entry(entry))
    };
    EngineRow { name: field.name.clone(), kind: field.kind.clone(), row, control }
}

/// Keeps the unsaved texts of the selected model before the page is redrawn.
fn capture_drafts(ui: &Ui) {
    let Some(id) = selected_id(ui) else { return };
    let mut drafts = ui.drafts.borrow_mut();
    drafts.insert(format!("tts_voice_instruction:{id}"), ui.voice.instruction.text().to_string());
    for row in ui.voice.engine.borrow().iter() {
        drafts.insert(format!("engine_draft:{id}:{}", row.name), row.text());
    }
}

fn selected_id(ui: &Ui) -> Option<String> {
    ui.catalog.borrow().selected(&ui.settings.borrow()).map(|m| m.id.clone())
}

pub(super) fn refresh(ui: &Rc<Ui>) {
    capture_drafts(ui);
    let generation = ui.catalog.borrow_mut().begin_refresh();
    render(ui);
    let ui = ui.clone();
    spawn(async move {
        let Some(client) = ui.client() else { return };
        if let Err(error) = client.discover_speech_models(generation).await {
            receive(&ui, &serde_json::json!({"generation": generation, "error": error}));
        }
    });
}

pub(super) fn receive(ui: &Rc<Ui>, payload: &Value) {
    capture_drafts(ui);
    let result = ui.catalog.borrow_mut().receive(payload);
    match result {
        None => {}
        Some(Err(error)) => {
            ui.set_status(&error);
            render(ui);
        }
        Some(Ok(())) => {
            render(ui);
            let ui = ui.clone();
            spawn(async move {
                let models = ui.catalog.borrow().models.clone();
                let settings = ui.settings.borrow().clone();
                if let Some(status) = form::adopt_catalog(&*ui, &models, &settings).await {
                    ui.set_status(&status);
                    render(&ui);
                }
            });
        }
    }
}

/// The instruction save, checked now so a later redraw cannot change what is saved.
fn instruction_save(ui: &Rc<Ui>) -> Pending {
    let id = selected_id(ui)?;
    let value = ui.voice.instruction.text().to_string();
    let key = match form::instruction_change(&ui.settings.borrow(), &id, &value) {
        Ok(key) => key?,
        Err(error) => {
            ui.set_status(&error);
            return None;
        }
    };
    let ui = ui.clone();
    Some(Box::pin(async move {
        ui.save(&key, &value).await;
    }))
}

fn engine_save(ui: &Rc<Ui>) -> Pending {
    let options = {
        let catalog = ui.catalog.borrow();
        let settings = ui.settings.borrow();
        let model = catalog.selected(&settings)?;
        let fields: Vec<_> =
            ui.voice.engine.borrow().iter().map(|r| (r.name.clone(), r.kind.clone(), r.text())).collect();
        let saved = settings.get("tts_engine_options").map_or("", String::as_str);
        form::engine_options(model, &fields, saved)
    };
    let options = match options {
        Ok(options) => options?,
        Err(error) => {
            ui.set_status(&error);
            return None;
        }
    };
    let ui = ui.clone();
    Some(Box::pin(async move {
        ui.save("tts_engine_options", &options).await;
    }))
}

fn commit_engine(ui: &Rc<Ui>) {
    if let Some(save) = engine_save(ui) {
        spawn(save);
    }
}

/// Typed text not saved yet, for the window close.
pub(super) fn pending(ui: &Rc<Ui>) -> Pending {
    let (instruction, engine) = (instruction_save(ui), engine_save(ui));
    if instruction.is_none() && engine.is_none() {
        return None;
    }
    Some(Box::pin(async move {
        for save in [instruction, engine].into_iter().flatten() {
            save.await;
        }
    }))
}

pub(super) fn connect(ui: &Rc<Ui>) {
    let page = &ui.voice;
    toggle(ui, &page.virtual_mic, "tts_virtual_mic", |_| {});
    let owner = ui.clone();
    page.model.connect_selected_notify(move |row| {
        if owner.updating.get() {
            return;
        }
        capture_drafts(&owner);
        let id = form::model_at(&owner.catalog.borrow(), row.selected() as usize);
        let ui = owner.clone();
        spawn(async move {
            ui.save("tts_model", &id).await;
            render(&ui);
        });
    });
    let owner = ui.clone();
    page.voice.connect_selected_notify(move |row| {
        if owner.updating.get() {
            return;
        }
        let selected = {
            let catalog = owner.catalog.borrow();
            let settings = owner.settings.borrow();
            catalog.selected(&settings).map(|m| (m.id.clone(), form::voice_at(m, row.selected() as usize)))
        };
        let Some((id, voice)) = selected else { return };
        let ui = owner.clone();
        spawn(async move {
            ui.save(&format!("tts_voice:{id}"), &voice).await;
        });
    });
    let owner = ui.clone();
    page.refresh.connect_clicked(move |_| {
        refresh(&owner);
        owner.set_status("Loading speech models...");
    });
    on_commit(ui, &page.instruction, |ui| {
        if let Some(save) = instruction_save(ui) {
            spawn(save);
        }
    });
    let owner = ui.clone();
    page.open_folder.connect_clicked(move |_| owner.set_status(&open_audio_folder()));
}

fn open_audio_folder() -> String {
    let folder = crate::paths::data_dir().join(dictto_core::speech::AUDIO_DIR);
    if let Err(error) = std::fs::create_dir_all(&folder) {
        return format!("Could not create the audio folder: {error}");
    }
    let uri = gio::File::for_path(&folder).uri();
    match gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
        Ok(()) => "Opened audio folder.".into(),
        Err(error) => format!("Could not open the audio folder: {error}"),
    }
}

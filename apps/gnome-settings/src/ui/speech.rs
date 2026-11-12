//! Speech: the model and voice discovered from the API, voice instruction, engine
//! options, the audio folder and the virtual microphone.
use super::{button, spawn, string_list, toggle, Ui};
use crate::form;
use adw::prelude::*;
use gtk::gio;
use serde_json::Value;
use std::rc::Rc;

pub(super) struct Page {
    pub page: adw::PreferencesPage,
    model: adw::ComboRow,
    refresh: gtk::Button,
    voice: adw::ComboRow,
    instruction: adw::EntryRow,
    instruction_save: gtk::Button,
    modes: adw::ActionRow,
    open_folder: gtk::Button,
    engine_group: adw::PreferencesGroup,
    engine: adw::EntryRow,
    engine_save: gtk::Button,
    virtual_mic: adw::SwitchRow,
}

impl Page {
    pub fn new() -> Self {
        let refresh = button("Refresh");
        let model = adw::ComboRow::builder().title("Speech model").build();
        model.add_suffix(&refresh);
        let voice = adw::ComboRow::builder().title("Engine voice").build();
        let instruction_save = button("Save");
        let instruction = adw::EntryRow::builder().title("Voice instruction").build();
        instruction.add_suffix(&instruction_save);
        let modes = adw::ActionRow::new();
        let open_folder = button("Open audio folder");
        let samples = adw::ActionRow::builder().title("Clone actions use your last recorded voice sample.").build();
        samples.add_suffix(&open_folder);
        let voices = adw::PreferencesGroup::new();
        for row in [model.upcast_ref::<gtk::Widget>(), voice.upcast_ref(), instruction.upcast_ref(), modes.upcast_ref(), samples.upcast_ref()] {
            voices.add(row);
        }
        let engine_save = button("Save");
        let engine = adw::EntryRow::builder().title("Options").build();
        engine.add_suffix(&engine_save);
        let engine_group = adw::PreferencesGroup::builder().title("Advanced engine options (JSON object)").build();
        engine_group.add(&engine);
        // Windows routes speech into VB-CABLE through the communications microphone; the
        // GNOME daemon makes its own PipeWire source the default input instead.
        let virtual_mic = adw::SwitchRow::builder()
            .title("Send speech to Dictto Microphone")
            .subtitle("Temporarily switches the default microphone during speech.")
            .build();
        let routing = adw::PreferencesGroup::new();
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
            instruction_save,
            modes,
            open_folder,
            engine_group,
            engine,
            engine_save,
            virtual_mic,
        }
    }
}

pub(super) fn render(ui: &Ui) {
    let page = &ui.speech;
    let view = form::speech_view(&ui.catalog.borrow(), &ui.settings.borrow(), &ui.drafts.borrow());
    ui.quietly(|| {
        page.model.set_model(Some(&string_list(&view.model_labels)));
        page.model.set_selected(view.model_index as u32);
        page.voice.set_model(Some(&string_list(&view.voice_labels)));
        page.voice.set_selected(view.voice_index as u32);
        page.instruction.set_text(&view.instruction);
        page.engine.set_text(&view.engine);
        page.virtual_mic.set_active(view.virtual_mic);
    });
    page.model.set_sensitive(view.model_enabled);
    page.refresh.set_sensitive(view.refresh_enabled);
    page.voice.set_sensitive(view.voice_enabled);
    page.instruction.set_sensitive(view.instruction_enabled);
    page.modes.set_title(&view.modes);
    page.engine.set_sensitive(view.engine_enabled);
    page.engine_group.set_description(Some(&view.engine_hint).filter(|hint| !hint.is_empty()).map(String::as_str));
}

/// Keeps the unsaved texts of the selected model before the page is redrawn.
fn capture_drafts(ui: &Ui) {
    let Some(id) = selected_id(ui) else { return };
    let mut drafts = ui.drafts.borrow_mut();
    drafts.insert(format!("tts_voice_instruction:{id}"), ui.speech.instruction.text().to_string());
    drafts.insert(format!("engine_draft:{id}"), ui.speech.engine.text().to_string());
}

fn selected_id(ui: &Ui) -> Option<String> {
    ui.catalog.borrow().selected(&ui.settings.borrow()).map(|m| m.id.clone())
}

/// Windows loads the catalog when its Speech page opens with no models yet.
pub(super) fn shown(ui: &Rc<Ui>) {
    let catalog = ui.catalog.borrow();
    if catalog.models.is_empty() && !catalog.loading {
        drop(catalog);
        refresh(ui);
    }
}

fn refresh(ui: &Rc<Ui>) {
    capture_drafts(ui);
    let generation = ui.catalog.borrow_mut().begin_refresh();
    ui.set_status(form::LOADING_MODELS);
    render(ui);
    let ui = ui.clone();
    spawn(async move {
        let Some(client) = ui.client() else { return };
        if let Err(error) = client.discover_speech_models(generation).await {
            ui.catalog.borrow_mut().loading = false;
            ui.set_status(&error);
            render(&ui);
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
                let status = form::adopt_catalog(&*ui, &models, &settings).await;
                ui.set_status(&status);
                render(&ui);
            });
        }
    }
}

pub(super) fn connect(ui: &Rc<Ui>) {
    let page = &ui.speech;
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
    page.refresh.connect_clicked(move |_| refresh(&owner));
    let owner = ui.clone();
    page.instruction_save.connect_clicked(move |_| {
        let Some(id) = selected_id(&owner) else { return };
        let (ui, value) = (owner.clone(), owner.speech.instruction.text().to_string());
        spawn(async move {
            let result = form::save_instruction(&*ui, &id, &value).await;
            ui.report(result);
        });
    });
    let owner = ui.clone();
    page.engine_save.connect_clicked(move |_| {
        let options = {
            let catalog = owner.catalog.borrow();
            let settings = owner.settings.borrow();
            let Some(model) = catalog.selected(&settings) else { return };
            let saved = settings.get("tts_engine_options").map_or("{}", String::as_str);
            form::engine_options(model, &owner.speech.engine.text(), saved)
        };
        match options {
            Ok(options) => {
                let ui = owner.clone();
                spawn(async move {
                    ui.save("tts_engine_options", &options).await;
                });
            }
            Err(error) => owner.set_status(&error),
        }
    });
    let owner = ui.clone();
    page.open_folder.connect_clicked(move |_| owner.set_status(&open_audio_folder()));
}

/// The core keeps samples and generated speech in "Speech Audio" under the data dir.
fn open_audio_folder() -> String {
    let folder = crate::paths::data_dir().join("Speech Audio");
    if let Err(error) = std::fs::create_dir_all(&folder) {
        return format!("Could not create the audio folder: {error}");
    }
    let uri = gio::File::for_path(&folder).uri();
    match gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
        Ok(()) => "Opened audio folder.".into(),
        Err(error) => format!("Could not open the audio folder: {error}"),
    }
}

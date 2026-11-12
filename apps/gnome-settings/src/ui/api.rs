//! API: base URL, API key, transcription and cleanup models.
use super::{button, spawn, speech, toggle, Ui};
use crate::form;
use adw::prelude::*;
use std::rc::Rc;

pub(super) struct Page {
    pub page: adw::PreferencesPage,
    url: adw::EntryRow,
    save_url: gtk::Button,
    key: adw::PasswordEntryRow,
    save_key: gtk::Button,
    remove_key: gtk::Button,
    warmup: adw::SwitchRow,
    transcription: adw::EntryRow,
    cleanup: adw::SwitchRow,
    llm: adw::EntryRow,
    save_models: gtk::Button,
}

impl Page {
    pub fn new() -> Self {
        let save_url = button("Save URL");
        let url = adw::EntryRow::builder().title("API base URL").build();
        url.add_suffix(&save_url);
        let (save_key, remove_key) = (button("Save key"), button("Remove key"));
        let key = adw::PasswordEntryRow::builder().title("API key").build();
        key.add_suffix(&save_key);
        key.add_suffix(&remove_key);
        let endpoint = adw::PreferencesGroup::new();
        endpoint.add(&url);
        endpoint.add(&key);
        let transcription = adw::EntryRow::builder().title("Transcription model").build();
        let warmup = adw::SwitchRow::builder()
            .title("Warm transcription model")
            .subtitle("Warm-up sends 1 second of silence; provider charges may apply.")
            .build();
        let llm = adw::EntryRow::builder().title("Cleanup model").build();
        let cleanup = adw::SwitchRow::builder()
            .title("Text cleanup")
            .subtitle("When cleanup is off, paste the original transcript.")
            .build();
        let save_models = button("Save models");
        let models = adw::PreferencesGroup::builder().title("Models").build();
        models.set_header_suffix(Some(&save_models));
        models.add(&transcription);
        models.add(&warmup);
        models.add(&llm);
        models.add(&cleanup);
        let page = adw::PreferencesPage::new();
        page.add(&endpoint);
        page.add(&models);
        Self { page, url, save_url, key, save_key, remove_key, warmup, transcription, cleanup, llm, save_models }
    }
}

pub(super) fn fill(ui: &Ui) {
    let page = &ui.api;
    let settings = ui.settings.borrow();
    let text = |key: &str| settings.get(key).cloned().unwrap_or_default();
    ui.quietly(|| {
        page.url.set_text(&text("api_base_url"));
        page.key.set_text("");
        page.transcription.set_text(&text("transcription_model"));
        page.llm.set_text(&text("llm_model"));
        page.warmup.set_active(form::is_on(&settings, "transcription_warmup"));
        page.cleanup.set_active(form::is_on(&settings, "cleanup_enabled"));
    });
    page.llm.set_sensitive(form::is_on(&settings, "cleanup_enabled"));
}

/// Windows shows the masked stored key as the empty field's cue text; an entry row has
/// no placeholder, so the hint joins the row title.
pub(super) async fn refresh_hint(ui: &Ui) {
    let Some(client) = ui.client() else { return };
    let hint = client.key_hint().await.unwrap_or_default();
    ui.api.key.set_title(&if hint.is_empty() { "API key".to_string() } else { format!("API key ({hint})") });
}

pub(super) fn connect(ui: &Rc<Ui>) {
    let page = &ui.api;
    toggle(ui, &page.warmup, "transcription_warmup", |_| {});
    toggle(ui, &page.cleanup, "cleanup_enabled", |ui| {
        ui.api.llm.set_sensitive(form::is_on(&ui.settings.borrow(), "cleanup_enabled"));
    });
    let owner = ui.clone();
    page.save_url.connect_clicked(move |_| {
        let (ui, value) = (owner.clone(), owner.api.url.text().to_string());
        spawn(async move {
            if ui.save("api_base_url", &value).await {
                ui.catalog.borrow_mut().invalidate();
                speech::render(&ui);
            }
        });
    });
    let owner = ui.clone();
    page.save_key.connect_clicked(move |_| {
        let (ui, value) = (owner.clone(), owner.api.key.text().to_string());
        spawn(async move {
            match form::save_key(&*ui, &value).await {
                Ok(text) => {
                    ui.api.key.set_text("");
                    refresh_hint(&ui).await;
                    ui.set_status(text);
                }
                Err(error) => ui.set_status(&error),
            }
        });
    });
    let owner = ui.clone();
    page.remove_key.connect_clicked(move |_| confirm_remove(&owner));
    let owner = ui.clone();
    page.save_models.connect_clicked(move |_| {
        let ui = owner.clone();
        let (transcription, llm) = (ui.api.transcription.text().to_string(), ui.api.llm.text().to_string());
        spawn(async move {
            let result = form::save_models(&*ui, &transcription, &llm).await;
            ui.report(result);
        });
    });
}

fn confirm_remove(ui: &Rc<Ui>) {
    let dialog = adw::AlertDialog::new(Some("Dictto"), Some("Remove the saved API key?"));
    dialog.add_responses(&[("no", "No"), ("yes", "Yes")]);
    dialog.set_response_appearance("yes", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("no"));
    dialog.set_close_response("no");
    let owner = ui.clone();
    dialog.connect_response(Some("yes"), move |_, _| {
        let ui = owner.clone();
        spawn(async move {
            match form::Backend::remove_key(&*ui).await {
                Ok(()) => {
                    refresh_hint(&ui).await;
                    ui.set_status(form::KEY_REMOVED);
                }
                Err(error) => ui.set_status(&error),
            }
        });
    });
    dialog.present(Some(&ui.window));
}

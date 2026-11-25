//! Connection: endpoint, API key, and whether the endpoint answered.
use super::{button, on_commit, spawn, Pending, Ui};
use crate::form;
use adw::prelude::*;
use std::rc::Rc;

pub(super) struct Page {
    pub page: adw::PreferencesPage,
    endpoint: adw::EntryRow,
    presets: gtk::Box,
    presets_popover: gtk::Popover,
    key: adw::PasswordEntryRow,
    save_key: gtk::Button,
    remove_key: gtk::Button,
    status: adw::ActionRow,
}

impl Page {
    pub fn new() -> Self {
        let presets = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let presets_popover = gtk::Popover::builder().child(&presets).build();
        let pick = gtk::MenuButton::builder()
            .icon_name("pan-down-symbolic")
            .tooltip_text("Endpoints")
            .popover(&presets_popover)
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let endpoint = adw::EntryRow::builder().title("Endpoint").build();
        endpoint.add_suffix(&pick);
        let server = adw::PreferencesGroup::builder()
            .description("Any OpenAI-compatible base URL, including /v1. Pick one or type your own.")
            .build();
        server.add(&endpoint);
        let (save_key, remove_key) = (button("Save"), button("Remove"));
        let key = adw::PasswordEntryRow::builder().title("API key").build();
        key.add_suffix(&save_key);
        key.add_suffix(&remove_key);
        // Windows keeps it in Credential Manager.
        let credentials = adw::PreferencesGroup::builder().description("Kept in GNOME Keyring.").build();
        credentials.add(&key);
        let status = adw::ActionRow::builder().title("Status").subtitle_selectable(true).build();
        let state = adw::PreferencesGroup::new();
        state.add(&status);
        let page = adw::PreferencesPage::new();
        page.add(&server);
        page.add(&credentials);
        page.add(&state);
        Self { page, endpoint, presets, presets_popover, key, save_key, remove_key, status }
    }
}

pub(super) fn fill(ui: &Rc<Ui>) {
    let page = &ui.connection;
    let settings = ui.settings.borrow();
    ui.quietly(|| {
        page.endpoint.set_text(settings.get("api_base_url").map_or("", String::as_str));
        page.key.set_text("");
    });
    while let Some(child) = page.presets.first_child() {
        page.presets.remove(&child);
    }
    for url in form::endpoint_presets(&settings) {
        let item = gtk::Button::builder().label(&url).css_classes(["flat"]).build();
        let owner = ui.clone();
        item.connect_clicked(move |_| {
            owner.connection.presets_popover.popdown();
            owner.quietly(|| owner.connection.endpoint.set_text(&url));
            commit_endpoint(&owner);
        });
        page.presets.append(&item);
    }
    drop(settings);
    show_status(ui);
}

pub(super) fn show_status(ui: &Ui) {
    let page = &ui.connection;
    page.status.set_subtitle(&ui.models.borrow().status(ui.has_key.get()));
    page.remove_key.set_sensitive(ui.has_key.get());
}

/// Windows shows the masked stored key as the empty field's cue text; an entry row has
/// no placeholder, so the hint joins the row title.
pub(super) async fn refresh_hint(ui: &Ui) {
    let Some(client) = ui.client() else { return };
    let hint = client.key_hint().await.unwrap_or_default();
    ui.set_has_key(!hint.is_empty());
    ui.connection.key.set_title(&if hint.is_empty() { "API key".to_string() } else { format!("API key ({hint})") });
    show_status(ui);
}

fn endpoint_save(ui: &Rc<Ui>) -> Pending {
    let url = form::endpoint_change(&ui.settings.borrow(), &ui.connection.endpoint.text())?;
    let ui = ui.clone();
    Some(Box::pin(async move {
        if ui.save("api_base_url", &url).await {
            ui.forget_catalogs();
            fill(&ui);
        }
    }))
}

fn commit_endpoint(ui: &Rc<Ui>) {
    if let Some(save) = endpoint_save(ui) {
        spawn(save);
    }
}

/// Typed text not saved yet, for the window close.
pub(super) fn pending(ui: &Rc<Ui>) -> Pending {
    endpoint_save(ui)
}

/// After the key changed, the lists belong to the previous key.
async fn key_changed(ui: &Rc<Ui>, status: &str) {
    ui.connection.key.set_text("");
    refresh_hint(ui).await;
    ui.forget_catalogs();
    ui.set_status(status);
}

pub(super) fn connect(ui: &Rc<Ui>) {
    let page = &ui.connection;
    on_commit(ui, &page.endpoint, commit_endpoint);
    let owner = ui.clone();
    page.save_key.connect_clicked(move |_| {
        let (ui, value) = (owner.clone(), owner.connection.key.text().to_string());
        spawn(async move {
            match form::save_key(&*ui, &value).await {
                Ok(text) => key_changed(&ui, text).await,
                Err(error) => ui.set_status(&error),
            }
        });
    });
    let owner = ui.clone();
    page.key.connect_entry_activated(move |_| owner.connection.save_key.emit_clicked());
    let owner = ui.clone();
    page.remove_key.connect_clicked(move |_| confirm_remove(&owner));
}

fn confirm_remove(ui: &Rc<Ui>) {
    let dialog = adw::AlertDialog::new(
        Some("Dictto"),
        Some("Remove the saved API key? Dictation stops working until you add one."),
    );
    dialog.add_responses(&[("no", "No"), ("yes", "Yes")]);
    dialog.set_response_appearance("yes", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("no"));
    dialog.set_close_response("no");
    let owner = ui.clone();
    dialog.connect_response(Some("yes"), move |_, _| {
        let ui = owner.clone();
        spawn(async move {
            match form::Backend::remove_key(&*ui).await {
                Ok(()) => key_changed(&ui, form::KEY_REMOVED).await,
                Err(error) => ui.set_status(&error),
            }
        });
    });
    dialog.present(Some(&ui.window));
}

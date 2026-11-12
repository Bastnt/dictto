//! Audio: microphone, recording controls, recording limit and paste delay.
use super::{button, entry, spawn, string_list, Ui};
use crate::bus::Bar;
use crate::form;
use adw::prelude::*;
use std::rc::Rc;

pub(super) struct Page {
    pub page: adw::PreferencesPage,
    microphone: adw::ComboRow,
    refresh: gtk::Button,
    record: gtk::Button,
    stop: gtk::Button,
    cancel: gtk::Button,
    limit: gtk::Entry,
    limit_save: gtk::Button,
    delay: gtk::Entry,
    delay_save: gtk::Button,
}

impl Page {
    pub fn new() -> Self {
        let refresh = button("Refresh");
        let microphone = adw::ComboRow::builder().title("Microphone").build();
        microphone.add_suffix(&refresh);
        let (record, stop, cancel) = (button("Record"), button("Stop"), button("Cancel"));
        let recording = adw::ActionRow::builder().title("Recording").build();
        recording.add_suffix(&record);
        recording.add_suffix(&stop);
        recording.add_suffix(&cancel);
        let devices = adw::PreferencesGroup::new();
        devices.add(&microphone);
        devices.add(&recording);
        let (limit, limit_save) = (entry(10), button("Save"));
        let limit_row = adw::ActionRow::builder().title("Recording limit (seconds)").build();
        limit_row.add_suffix(&limit);
        limit_row.add_suffix(&limit_save);
        let (delay, delay_save) = (entry(10), button("Save"));
        let delay_row = adw::ActionRow::builder().title("Paste delay (milliseconds)").build();
        delay_row.add_suffix(&delay);
        delay_row.add_suffix(&delay_save);
        let timing = adw::PreferencesGroup::new();
        timing.add(&limit_row);
        timing.add(&delay_row);
        let page = adw::PreferencesPage::new();
        page.add(&devices);
        page.add(&timing);
        Self { page, microphone, refresh, record, stop, cancel, limit, limit_save, delay, delay_save }
    }
}

pub(super) fn fill(ui: &Ui) {
    let page = &ui.audio;
    let settings = ui.settings.borrow();
    page.limit.set_text(settings.get("recording_limit_seconds").map_or("", String::as_str));
    page.delay.set_text(settings.get("paste_delay_ms").map_or("", String::as_str));
    // As on Windows, a preview daemon has no recording services to drive.
    for button in [&page.record, &page.stop, &page.cancel] {
        button.set_sensitive(!ui.preview.get());
    }
}

pub(super) async fn refresh_microphones(ui: &Ui) {
    let Some(client) = ui.client() else { return };
    let list = match client.microphones().await {
        Ok(list) => list,
        Err(error) => {
            ui.set_status(&error);
            Vec::new()
        }
    };
    let choices = form::microphone_choices(&list, &ui.settings.borrow());
    ui.quietly(|| {
        ui.audio.microphone.set_model(Some(&string_list(&choices.labels)));
        ui.audio.microphone.set_selected(choices.selected as u32);
    });
    *ui.microphones.borrow_mut() = choices.values;
}

pub(super) fn connect(ui: &Rc<Ui>) {
    let page = &ui.audio;
    let owner = ui.clone();
    page.microphone.connect_selected_notify(move |row| {
        if owner.updating.get() {
            return;
        }
        let Some(value) = owner.microphones.borrow().get(row.selected() as usize).cloned() else { return };
        let ui = owner.clone();
        spawn(async move {
            ui.save("microphone_device", &value).await;
        });
    });
    let owner = ui.clone();
    page.refresh.connect_clicked(move |_| {
        let ui = owner.clone();
        spawn(async move { refresh_microphones(&ui).await });
    });
    for (button, action) in [(&page.record, Bar::Start), (&page.stop, Bar::Stop), (&page.cancel, Bar::Cancel)] {
        let owner = ui.clone();
        button.connect_clicked(move |_| {
            let ui = owner.clone();
            spawn(async move {
                let Some(client) = ui.client() else { return };
                if let Err(error) = client.bar(action).await {
                    ui.set_status(&error);
                }
            });
        });
    }
    for (save, field, key) in [
        (&page.limit_save, &page.limit, "recording_limit_seconds"),
        (&page.delay_save, &page.delay, "paste_delay_ms"),
    ] {
        let (owner, field) = (ui.clone(), field.clone());
        save.connect_clicked(move |_| {
            let (ui, value) = (owner.clone(), field.text().to_string());
            spawn(async move {
                ui.save(key, &value).await;
            });
        });
    }
}

//! Dictation: transcription and cleanup models, warm-up, recording limit and paste delay.
use super::{button, toggle, List, Ui};
use crate::form;
use adw::prelude::*;
use std::rc::Rc;

pub(super) struct Page {
    pub page: adw::PreferencesPage,
    transcription: List,
    refresh: gtk::Button,
    warmup: adw::SwitchRow,
    cleanup: adw::SwitchRow,
    llm: List,
    limit: List,
    delay: List,
}

impl Page {
    pub fn new() -> Self {
        let transcription = List::new("Transcription model", None);
        let refresh = button("Refresh");
        transcription.row.add_suffix(&refresh);
        let warmup = adw::SwitchRow::builder()
            .title("Warm up the model so the first words come back faster")
            .subtitle("Sends 1 second of silence at startup and when recording starts; provider charges may apply.")
            .build();
        let speech = adw::PreferencesGroup::new();
        speech.add(&transcription.row);
        speech.add(&warmup);
        let cleanup = adw::SwitchRow::builder().title("Fix punctuation and remove filler words").build();
        let llm = List::new("Cleanup model", Some("When cleanup is off, the raw transcript is pasted."));
        let text = adw::PreferencesGroup::builder().title("Text cleanup").build();
        text.add(&cleanup);
        text.add(&llm.row);
        let limit = List::new("Maximum recording", None);
        let delay = List::new("Paste delay", Some("Raise it if the start of pasted text goes missing."));
        let timing = adw::PreferencesGroup::new();
        timing.add(&limit.row);
        timing.add(&delay.row);
        let page = adw::PreferencesPage::new();
        page.add(&speech);
        page.add(&text);
        page.add(&timing);
        Self { page, transcription, refresh, warmup, cleanup, llm, limit, delay }
    }
}

pub(super) fn fill(ui: &Ui) {
    let page = &ui.dictation;
    let settings = ui.settings.borrow();
    let models = ui.models.borrow();
    page.transcription.fill(ui, models.choices(&models.transcription, &settings, "transcription_model"));
    page.llm.fill(ui, models.choices(&models.chat, &settings, "llm_model"));
    page.limit.fill(ui, form::preset_choices(form::LIMITS, &settings, "recording_limit_seconds", "seconds"));
    page.delay.fill(ui, form::preset_choices(form::DELAYS, &settings, "paste_delay_ms", "ms"));
    page.refresh.set_label(if models.loading { "Loading..." } else { "Refresh" });
    page.refresh.set_sensitive(!models.loading && ui.has_key.get());
    ui.quietly(|| {
        page.warmup.set_active(form::is_on(&settings, "transcription_warmup"));
        page.cleanup.set_active(form::is_on(&settings, "cleanup_enabled"));
    });
    page.llm.row.set_sensitive(form::is_on(&settings, "cleanup_enabled"));
}

pub(super) fn connect(ui: &Rc<Ui>) {
    let page = &ui.dictation;
    toggle(ui, &page.warmup, "transcription_warmup", |_| {});
    toggle(ui, &page.cleanup, "cleanup_enabled", |ui| {
        ui.dictation.llm.row.set_sensitive(form::is_on(&ui.settings.borrow(), "cleanup_enabled"));
    });
    List::bind(ui, |ui| &ui.dictation.transcription, "transcription_model", fill);
    List::bind(ui, |ui| &ui.dictation.llm, "llm_model", fill);
    List::bind(ui, |ui| &ui.dictation.limit, "recording_limit_seconds", fill);
    List::bind(ui, |ui| &ui.dictation.delay, "paste_delay_ms", fill);
    let owner = ui.clone();
    page.refresh.connect_clicked(move |_| {
        owner.load_models(true);
        owner.set_status("Loading models from the endpoint...");
    });
}

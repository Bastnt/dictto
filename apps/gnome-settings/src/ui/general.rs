//! General: sign-in start, sounds, spoken languages and the microphone.
use super::{button, spawn, toggle, List, Ui};
use crate::form;
use adw::prelude::*;
use dictto_core::settings_model::LANGUAGES;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(super) struct Page {
    pub page: adw::PreferencesPage,
    auto_start: adw::SwitchRow,
    sounds: adw::SwitchRow,
    languages: gtk::Label,
    choose: gtk::Button,
    microphone: List,
}

impl Page {
    pub fn new() -> Self {
        // Windows says "when you sign in to Windows".
        let auto_start = adw::SwitchRow::builder().title("Start Dictto when you sign in").build();
        let sounds = adw::SwitchRow::builder().title("Play a sound when recording starts and stops").build();
        let startup = adw::PreferencesGroup::new();
        startup.add(&auto_start);
        startup.add(&sounds);
        let languages = gtk::Label::builder().wrap(true).xalign(1.0).build();
        let choose = button("Choose...");
        let row = adw::ActionRow::builder()
            .title("Spoken languages")
            .subtitle("Naming your languages helps the model; auto-detect guesses for each dictation.")
            .build();
        row.add_suffix(&languages);
        row.add_suffix(&choose);
        let microphone =
            List::new("Microphone", Some("Auto-detect follows the system default input device."));
        let input = adw::PreferencesGroup::new();
        input.add(&row);
        input.add(&microphone.row);
        let page = adw::PreferencesPage::new();
        page.add(&startup);
        page.add(&input);
        Self { page, auto_start, sounds, languages, choose, microphone }
    }
}

pub(super) fn fill(ui: &Ui) {
    let page = &ui.general;
    let settings = ui.settings.borrow();
    ui.quietly(|| {
        page.auto_start.set_active(form::is_on(&settings, "auto_start"));
        page.sounds.set_active(form::is_on(&settings, "sound_effects_enabled"));
    });
    page.languages.set_text(&form::languages_summary(&settings));
    drop(settings);
    fill_microphones(ui);
}

fn fill_microphones(ui: &Ui) {
    let choices = form::microphone_choices(&ui.microphones.borrow(), &ui.settings.borrow());
    ui.general.microphone.fill(ui, choices);
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
    *ui.microphones.borrow_mut() = list;
    fill_microphones(ui);
}

pub(super) fn connect(ui: &Rc<Ui>) {
    let page = &ui.general;
    toggle(ui, &page.auto_start, "auto_start", |_| {});
    toggle(ui, &page.sounds, "sound_effects_enabled", |_| {});
    List::bind(ui, |ui| &ui.general.microphone, "microphone_device", fill_microphones);
    let owner = ui.clone();
    page.choose.connect_clicked(move |_| open_languages(&owner));
}

struct LanguageRow {
    code: &'static str,
    name: &'static str,
    row: adw::ActionRow,
    check: gtk::CheckButton,
}

/// The Windows "Spoken languages (select all that apply)" page, as a dialog. Each change
/// is saved at once, as on Windows.
fn open_languages(ui: &Rc<Ui>) {
    let auto = gtk::CheckButton::with_label("Auto-detect");
    let search = gtk::SearchEntry::builder().placeholder_text("Language name").hexpand(true).build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let rows: Rc<Vec<LanguageRow>> = Rc::new(
        LANGUAGES
            .iter()
            .map(|&(code, name)| {
                let check = gtk::CheckButton::new();
                let row = adw::ActionRow::builder().title(name).activatable_widget(&check).build();
                row.add_prefix(&check);
                list.append(&row);
                LanguageRow { code, name, row, check }
            })
            .collect(),
    );
    // Selection as shown; the setting is written from it so quick clicks do not race.
    let selected = Rc::new(RefCell::new(Vec::<String>::new()));
    let filling = Rc::new(Cell::new(false));
    let sync = {
        let (ui, rows, auto, selected, filling) = (ui.clone(), rows.clone(), auto.clone(), selected.clone(), filling.clone());
        Rc::new(move || {
            let settings = ui.settings.borrow();
            *selected.borrow_mut() = form::languages(&settings).into_iter().filter(|v| v != "auto").collect();
            filling.set(true);
            auto.set_active(form::auto_detect(&settings));
            for row in rows.iter() {
                row.check.set_active(selected.borrow().iter().any(|v| v == row.code));
            }
            filling.set(false);
            ui.general.languages.set_text(&form::languages_summary(&settings));
        })
    };
    sync();
    let save = {
        let (ui, sync) = (ui.clone(), sync.clone());
        Rc::new(move |value: String| {
            let (ui, sync) = (ui.clone(), sync.clone());
            spawn(async move {
                if ui.save("languages", &value).await {
                    ui.general.languages.set_text(&form::languages_summary(&ui.settings.borrow()));
                } else {
                    sync();
                }
            });
        })
    };
    for row in rows.iter() {
        let (all, selected, filling, auto, save) =
            // Weak: the rows own this handler.
            (Rc::downgrade(&rows), selected.clone(), filling.clone(), auto.clone(), save.clone());
        row.check.connect_toggled(move |_| {
            let Some(all) = all.upgrade() else { return };
            if filling.get() {
                return;
            }
            let visible: Vec<&LanguageRow> = all.iter().filter(|r| r.row.is_visible()).collect();
            let codes: Vec<&str> = visible.iter().map(|r| r.code).collect();
            let checked: Vec<&str> = visible.iter().filter(|r| r.check.is_active()).map(|r| r.code).collect();
            let values = form::selected_languages(&selected.borrow(), &codes, &checked);
            *selected.borrow_mut() = values.clone();
            filling.set(true);
            auto.set_active(values.is_empty());
            filling.set(false);
            save(form::languages_value(&values));
        });
    }
    {
        let (ui, rows, selected, filling, save) = (ui.clone(), rows.clone(), selected.clone(), filling.clone(), save.clone());
        auto.connect_toggled(move |auto| {
            if filling.get() {
                return;
            }
            if auto.is_active() {
                selected.borrow_mut().clear();
                filling.set(true);
                for row in rows.iter() {
                    row.check.set_active(false);
                }
                filling.set(false);
                save("[]".into());
            } else {
                ui.set_status("Select one or more languages below.");
            }
        });
    }
    {
        let rows = rows.clone();
        search.connect_search_changed(move |search| {
            let filter = search.text();
            for row in rows.iter() {
                row.row.set_visible(form::language_matches(row.code, row.name, &filter));
            }
        });
    }
    let filters = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    filters.append(&auto);
    filters.append(&search);
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    body.append(&gtk::Label::builder().label("Spoken languages (select all that apply)").xalign(0.0).build());
    body.append(&filters);
    body.append(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).build());
    let done = gtk::Button::builder().label("Done").css_classes(["suggested-action"]).build();
    let header = adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();
    header.pack_end(&done);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));
    let dialog = adw::Dialog::builder()
        .title("Spoken languages")
        .content_width(460)
        .content_height(600)
        .child(&toolbar)
        .build();
    let closing = dialog.clone();
    done.connect_clicked(move |_| {
        closing.close();
    });
    dialog.present(Some(&ui.window));
    search.grab_focus();
}

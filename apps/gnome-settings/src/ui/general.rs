//! General: start on boot, sound effects, languages and the push-to-talk hotkey.
use super::{button, shortcuts, spawn, toggle, Ui};
use crate::form;
use adw::prelude::*;
use dictto_core::settings_model::LANGUAGES;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(super) struct Page {
    pub page: adw::PreferencesPage,
    auto_start: adw::SwitchRow,
    sounds: adw::SwitchRow,
    languages: adw::ActionRow,
    change: gtk::Button,
    hotkey: gtk::Entry,
    save: gtk::Button,
}

impl Page {
    pub fn new() -> Self {
        let auto_start =
            adw::SwitchRow::builder().title("Start on boot").subtitle("Launch Dictto when you sign in").build();
        let sounds =
            adw::SwitchRow::builder().title("Sound effects").subtitle("Play sounds on recording events").build();
        let change = button("Change");
        let languages = adw::ActionRow::builder().title("Languages").build();
        languages.add_suffix(&change);
        let options = adw::PreferencesGroup::new();
        options.add(&auto_start);
        options.add(&sounds);
        options.add(&languages);
        let hotkey = shortcuts::field();
        let save = button("Save");
        let row = adw::ActionRow::builder()
            .title("Push-to-talk hotkey")
            .subtitle("Hold to dictate; double-press for continuous recording. Keys are named by their US-layout position.")
            .build();
        row.add_suffix(&hotkey);
        row.add_suffix(&save);
        let dictation = adw::PreferencesGroup::new();
        dictation.add(&row);
        let page = adw::PreferencesPage::new();
        page.add(&options);
        page.add(&dictation);
        Self { page, auto_start, sounds, languages, change, hotkey, save }
    }
}

pub(super) fn fill(ui: &Ui) {
    let page = &ui.general;
    let settings = ui.settings.borrow();
    ui.quietly(|| {
        page.auto_start.set_active(form::is_on(&settings, "auto_start"));
        page.sounds.set_active(form::is_on(&settings, "sound_effects_enabled"));
        page.languages.set_subtitle(&form::languages_summary(&settings));
        page.hotkey.set_text(settings.get("hotkey").map_or("", String::as_str));
    });
}

pub(super) fn connect(ui: &Rc<Ui>) {
    let page = &ui.general;
    toggle(ui, &page.auto_start, "auto_start", |_| {});
    toggle(ui, &page.sounds, "sound_effects_enabled", |_| {});
    shortcuts::record(ui, &page.hotkey);
    let owner = ui.clone();
    page.change.connect_clicked(move |_| open_languages(&owner));
    let owner = ui.clone();
    page.save.connect_clicked(move |_| {
        let ui = owner.clone();
        spawn(async move {
            let raw = ui.general.hotkey.text();
            let settings = ui.settings.borrow().clone();
            match form::save_shortcut(&*ui, &settings, "hotkey", &raw).await {
                Ok(value) => {
                    ui.general.hotkey.set_text(&value);
                    ui.set_status(form::SAVED);
                }
                Err(error) => ui.set_status(&error),
            }
        });
    });
}

struct LanguageRow {
    code: &'static str,
    name: &'static str,
    row: adw::ActionRow,
    check: gtk::CheckButton,
}

/// The Windows "Languages (select all that apply)" page, as a dialog. Each change is
/// saved at once, as on Windows.
fn open_languages(ui: &Rc<Ui>) {
    let auto = gtk::CheckButton::with_label("Auto-detect");
    let search = gtk::SearchEntry::builder().placeholder_text("Search").hexpand(true).build();
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
            ui.general.languages.set_subtitle(&form::languages_summary(&settings));
        })
    };
    sync();
    let save = {
        let (ui, sync) = (ui.clone(), sync.clone());
        Rc::new(move |value: String| {
            let (ui, sync) = (ui.clone(), sync.clone());
            spawn(async move {
                if ui.save("languages", &value).await {
                    ui.general.languages.set_subtitle(&form::languages_summary(&ui.settings.borrow()));
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
    body.append(&gtk::Label::builder().label("Languages (select all that apply)").xalign(0.0).build());
    body.append(&filters);
    body.append(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).build());
    let done = gtk::Button::builder().label("Done").css_classes(["suggested-action"]).build();
    let header = adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();
    header.pack_end(&done);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));
    let dialog = adw::Dialog::builder()
        .title("Languages")
        .content_width(460)
        .content_height(600)
        .child(&toolbar)
        .build();
    let closing = dialog.clone();
    done.connect_clicked(move |_| {
        closing.close();
    });
    dialog.present(Some(&ui.window));
}

//! Shortcuts: the speech shortcuts, and the key-recording field the hotkey shares.
use super::{button, spawn, Ui};
use crate::chord::{ChordRecorder, Recorded};
use crate::form::{self, SPEECH_SHORTCUTS};
use adw::prelude::*;
use dictto_core::keys::Key;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) struct Page {
    pub page: adw::PreferencesPage,
    fields: Vec<(&'static str, gtk::Entry, gtk::Button)>,
}

impl Page {
    pub fn new() -> Self {
        let group = adw::PreferencesGroup::builder()
            .title("Speech shortcuts")
            .description("Enter a chord such as Ctrl+Alt+F8. Clear the field to disable it.")
            .build();
        let fields = SPEECH_SHORTCUTS
            .iter()
            .map(|&(key, label)| {
                let (field, save) = (field(), button("Save"));
                let row = adw::ActionRow::builder().title(label).build();
                row.add_suffix(&field);
                row.add_suffix(&save);
                group.add(&row);
                (key, field, save)
            })
            .collect();
        let note = adw::PreferencesGroup::builder()
            .description("Read text uses the selection in the focused app, then the clipboard if nothing is selected.")
            .build();
        let page = adw::PreferencesPage::new();
        page.add(&group);
        page.add(&note);
        Self { page, fields }
    }
}

pub(super) fn fill(ui: &Ui) {
    let settings = ui.settings.borrow();
    for (key, field, _) in &ui.shortcuts.fields {
        field.set_text(settings.get(*key).map_or("", String::as_str));
    }
}

pub(super) fn connect(ui: &Rc<Ui>) {
    for (key, field, save) in &ui.shortcuts.fields {
        record(ui, field);
        let (owner, key, field) = (ui.clone(), *key, field.clone());
        save.connect_clicked(move |_| {
            let (ui, field) = (owner.clone(), field.clone());
            spawn(async move {
                let settings = ui.settings.borrow().clone();
                match form::save_shortcut(&*ui, &settings, key, &field.text()).await {
                    Ok(value) => {
                        field.set_text(&value);
                        ui.set_status(form::SAVED);
                    }
                    Err(error) => ui.set_status(&error),
                }
            });
        });
    }
}

/// A field that shows the chord pressed in it rather than typed text.
pub(super) fn field() -> gtk::Entry {
    let field = gtk::Entry::builder()
        .width_chars(16)
        .valign(gtk::Align::Center)
        .placeholder_text("Press keys")
        .build();
    field.set_editable(false);
    field
}

/// GDK hardware keycodes are evdev codes plus 8 under both Wayland and X11.
fn physical_key(keycode: u32) -> Option<Key> {
    crate::keymap::from_evdev(u16::try_from(keycode.checked_sub(8)?).ok()?)
}

/// Records chords in `field`. While it has focus the daemon's shortcuts are suspended
/// (SetHotkeyCapture), so pressing the current hotkey here does not start dictation.
pub(super) fn record(ui: &Rc<Ui>, field: &gtk::Entry) {
    let recorder = Rc::new(RefCell::new(ChordRecorder::default()));
    let show = {
        let (ui, field) = (ui.clone(), field.clone());
        move |result: Recorded| match result {
            Recorded::Pending => {}
            Recorded::Chord(text) => field.set_text(&text),
            Recorded::Clear => field.set_text(""),
            Recorded::Invalid(error) => ui.set_status(&error),
        }
    };
    let show = Rc::new(show);
    let keys = gtk::EventControllerKey::new();
    // Ahead of the entry's own key handling.
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (recorder, show) = (recorder.clone(), show.clone());
        keys.connect_key_pressed(move |_, _, keycode, _| {
            let Some(key) = physical_key(keycode) else { return glib::Propagation::Proceed };
            if key == Key::Tab && !recorder.borrow().holds_modifier() {
                return glib::Propagation::Proceed;
            }
            let result = recorder.borrow_mut().press(key);
            show(result);
            glib::Propagation::Stop
        });
    }
    {
        let recorder = recorder.clone();
        keys.connect_key_released(move |_, _, keycode, _| {
            if let Some(key) = physical_key(keycode) {
                let result = recorder.borrow_mut().release(key);
                show(result);
            }
        });
    }
    field.add_controller(keys);
    let focus = gtk::EventControllerFocus::new();
    {
        let (ui, recorder) = (ui.clone(), recorder.clone());
        focus.connect_enter(move |_| {
            recorder.borrow_mut().reset();
            ui.set_field_focus(true);
        });
    }
    {
        let ui = ui.clone();
        focus.connect_leave(move |_| {
            recorder.borrow_mut().reset();
            ui.set_field_focus(false);
        });
    }
    field.add_controller(focus);
}

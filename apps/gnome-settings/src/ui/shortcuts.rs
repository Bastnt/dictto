//! Shortcuts: dictation and speech shortcuts, each field recording the keys pressed in it
//! and saving the chord once they are released.
use super::{button, spawn, Ui};
use crate::chord::{ChordRecorder, Recorded};
use crate::form::{self, SHORTCUTS};
use adw::prelude::*;
use dictto_core::keys::Key;
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) struct Page {
    pub page: adw::PreferencesPage,
    fields: Vec<(gtk::Entry, gtk::Button)>,
}

impl Page {
    pub fn new() -> Self {
        let group = adw::PreferencesGroup::builder()
            // Windows has no Backspace line: a Windows field takes Backspace as a key.
            // Keys are recorded by physical position, so the layout note matters here.
            .description(
                "Click a field, then press the keys together. Escape cancels; Backspace clears. \
                 Keys are named by their US-layout position.",
            )
            .build();
        let fields = SHORTCUTS
            .iter()
            .enumerate()
            .map(|(index, &(_, label))| {
                let field = gtk::Entry::builder()
                    .width_chars(16)
                    .valign(gtk::Align::Center)
                    .placeholder_text("Not set")
                    .build();
                field.set_editable(false);
                let reset = button(if index == 0 { "Reset" } else { "Clear" });
                let row = adw::ActionRow::builder().title(label).build();
                row.add_suffix(&field);
                row.add_suffix(&reset);
                group.add(&row);
                (field, reset)
            })
            .collect();
        let note = adw::PreferencesGroup::builder()
            .description(
                "Read text uses the selection in the focused app, or the clipboard when nothing is selected. \
                 For recording shortcuts, hold to record or double-press to keep recording. \
                 Press again to stop. Clone actions use your last voice sample.",
            )
            .build();
        let page = adw::PreferencesPage::new();
        page.add(&group);
        page.add(&note);
        Self { page, fields }
    }
}

pub(super) fn fill(ui: &Ui) {
    for index in 0..SHORTCUTS.len() {
        restore(ui, index);
    }
}

fn restore(ui: &Ui, index: usize) {
    let value = ui.settings.borrow().get(SHORTCUTS[index].0).cloned().unwrap_or_default();
    ui.shortcuts.fields[index].0.set_text(&value);
}

pub(super) fn connect(ui: &Rc<Ui>) {
    for (index, (field, reset)) in ui.shortcuts.fields.iter().enumerate() {
        record(ui, index, field);
        let owner = ui.clone();
        reset.connect_clicked(move |_| {
            let ui = owner.clone();
            spawn(async move {
                let key = SHORTCUTS[index].0;
                ui.save(key, form::shortcut_reset(key)).await;
                restore(&ui, index);
            });
        });
    }
}

/// GDK hardware keycodes are evdev codes plus 8 under both Wayland and X11. The daemon
/// reads evdev codes, so a key is stored by its US-layout position whatever the layout.
fn physical_key(keycode: u32) -> Option<Key> {
    crate::keymap::from_evdev(u16::try_from(keycode.checked_sub(8)?).ok()?)
}

/// Records chords in field `index`. While it has focus the daemon's shortcuts are
/// suspended (SetHotkeyCapture), so pressing the current hotkey here does not start
/// dictation.
fn record(ui: &Rc<Ui>, index: usize, field: &gtk::Entry) {
    let recorder = Rc::new(RefCell::new(ChordRecorder::default()));
    // The chord shown, saved once every key is up.
    let recorded = Rc::new(RefCell::new(None::<String>));
    let show = {
        let (ui, field, recorded) = (ui.clone(), field.clone(), recorded.clone());
        Rc::new(move |result: Recorded| match result {
            Recorded::Pending => {}
            Recorded::Chord(text) => {
                field.set_text(&text);
                *recorded.borrow_mut() = Some(text);
            }
            Recorded::Clear => {
                field.set_text("");
                *recorded.borrow_mut() = Some(String::new());
            }
            Recorded::Invalid(error) => {
                *recorded.borrow_mut() = None;
                ui.set_status(&error);
            }
        })
    };
    let keys = gtk::EventControllerKey::new();
    // Ahead of the entry's own key handling.
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (ui, recorder, recorded, show) = (ui.clone(), recorder.clone(), recorded.clone(), show.clone());
        keys.connect_key_pressed(move |_, keyval, keycode, _| {
            if keyval == gdk::Key::Escape {
                recorder.borrow_mut().reset();
                recorded.borrow_mut().take();
                restore(&ui, index);
                ui.shortcuts.fields[index].1.grab_focus();
                return glib::Propagation::Stop;
            }
            let Some(key) = physical_key(keycode) else { return glib::Propagation::Proceed };
            if key == Key::Tab && !recorder.borrow().holds_shortcut_modifier() {
                return glib::Propagation::Proceed;
            }
            let result = recorder.borrow_mut().press(key);
            show(result);
            glib::Propagation::Stop
        });
    }
    {
        let (ui, recorder, recorded) = (ui.clone(), recorder.clone(), recorded.clone());
        keys.connect_key_released(move |_, _, keycode, _| {
            let Some(key) = physical_key(keycode) else { return };
            let result = recorder.borrow_mut().release(key);
            show(result);
            if !recorder.borrow().idle() {
                return;
            }
            let Some(raw) = recorded.borrow_mut().take() else { return };
            let ui = ui.clone();
            spawn(async move {
                let key = SHORTCUTS[index].0;
                let settings = ui.settings.borrow().clone();
                match form::save_shortcut(&*ui, &settings, key, &raw).await {
                    Ok(_) => {
                        ui.set_status(form::SAVED);
                        ui.shortcuts.fields[index].1.grab_focus();
                    }
                    Err(error) => ui.set_status(&error),
                }
                restore(&ui, index);
            });
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
            recorded.borrow_mut().take();
            ui.set_field_focus(false);
            restore(&ui, index);
        });
    }
    field.add_controller(focus);
}

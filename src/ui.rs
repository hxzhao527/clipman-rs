use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gtk::prelude::*;
use gtk::{
    AlertDialog, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerMotion,
    HeaderBar, Label, ListBox, ListBoxRow, Orientation, ScrolledWindow, SearchEntry, SpinButton,
    Switch,
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

use crate::clipboard::{ClipboardService, ClipboardSource};
use crate::domain::{CapturedPayload, menu_preview};
use crate::i18n::Translator;
use crate::storage::{HistorySnapshot, Settings, StorageService};
use crate::tray::{ClipmanTray, TrayCommand, TrayItem, TraySnapshot, TrayTarget};

#[derive(Clone, Debug)]
pub enum StorageEvent {
    Snapshot(HistorySnapshot),
    Error(String),
}

#[derive(Clone)]
pub struct AppUi {
    inner: Rc<UiInner>,
}

pub struct AppUiInit {
    pub app: Application,
    pub tray_snapshot: Arc<Mutex<TraySnapshot>>,
    pub tray_handle: ksni::Handle<ClipmanTray>,
    pub tokio_handle: tokio::runtime::Handle,
    pub clipboard: ClipboardService,
    pub storage: StorageService,
    pub storage_events: UnboundedSender<StorageEvent>,
    pub initial_snapshot: HistorySnapshot,
    pub translator: Translator,
}

#[derive(Clone)]
struct SettingsControls {
    primary: Switch,
    max_history: SpinButton,
    max_menu: SpinButton,
}

struct UiInner {
    app: Application,
    state: RefCell<HistorySnapshot>,
    window: RefCell<Option<ApplicationWindow>>,
    list: RefCell<Option<ListBox>>,
    settings_window: RefCell<Option<ApplicationWindow>>,
    settings_controls: RefCell<Option<SettingsControls>>,
    settings_loading: Cell<bool>,
    search_query: RefCell<String>,
    tray_snapshot: Arc<Mutex<TraySnapshot>>,
    tray_handle: ksni::Handle<ClipmanTray>,
    tokio_handle: tokio::runtime::Handle,
    clipboard: ClipboardService,
    storage: StorageService,
    storage_events: UnboundedSender<StorageEvent>,
    translator: Translator,
}

enum StorageOperation {
    Upsert(ClipboardSource, CapturedPayload),
    Touch(u64),
    Delete(u64),
    ClearRegular,
    UpdateSettings(Settings),
}

impl AppUi {
    pub fn new(init: AppUiInit) -> Self {
        let ui = Self {
            inner: Rc::new(UiInner {
                app: init.app,
                state: RefCell::new(init.initial_snapshot),
                window: RefCell::new(None),
                list: RefCell::new(None),
                settings_window: RefCell::new(None),
                settings_controls: RefCell::new(None),
                settings_loading: Cell::new(false),
                search_query: RefCell::new(String::new()),
                tray_snapshot: init.tray_snapshot,
                tray_handle: init.tray_handle,
                tokio_handle: init.tokio_handle,
                clipboard: init.clipboard,
                storage: init.storage,
                storage_events: init.storage_events,
                translator: init.translator,
            }),
        };
        ui.publish();
        ui
    }

    pub fn add_capture(&self, source: ClipboardSource, payload: CapturedPayload) {
        info!(
            ?source,
            text_length = payload.plain_text.chars().count(),
            "adding clipboard capture"
        );
        self.spawn_storage_operation(StorageOperation::Upsert(source, payload));
    }

    pub fn handle_storage_event(&self, event: StorageEvent) {
        match event {
            StorageEvent::Snapshot(snapshot) => {
                self.inner
                    .clipboard
                    .set_primary_enabled(snapshot.settings.primary_enabled);
                *self.inner.state.borrow_mut() = snapshot;
                self.sync_settings_controls();
                self.publish();
            }
            StorageEvent::Error(error) => {
                warn!(%error, "clipboard storage operation failed");
                self.show_error(&error);
            }
        }
    }

    pub fn handle_command(&self, command: TrayCommand) {
        match command {
            TrayCommand::Activate(target) => self.activate(target),
            TrayCommand::ShowAll => self.show(),
            TrayCommand::Clear => self.request_clear_regular(),
            TrayCommand::Settings => self.show_settings(),
            TrayCommand::Exit => self.inner.app.quit(),
        }
    }

    pub fn show(&self) {
        if let Some(window) = self.inner.window.borrow().as_ref() {
            window.present();
            return;
        }

        let window = ApplicationWindow::builder()
            .application(&self.inner.app)
            .title(self.inner.translator.text("history.window_title"))
            .default_width(760)
            .default_height(520)
            .build();
        let header = HeaderBar::new();
        let header_title = Label::new(Some(&self.inner.translator.text("history.window_title")));
        header.set_title_widget(Some(&header_title));
        window.set_titlebar(Some(&header));

        let content = GtkBox::new(Orientation::Vertical, 8);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);

        let title = Label::new(Some(&self.inner.translator.text("history.heading")));
        title.set_xalign(0.0);
        title.add_css_class("title-1");
        content.append(&title);

        let search = SearchEntry::new();
        search.set_placeholder_text(Some(
            &self.inner.translator.text("history.search_placeholder"),
        ));
        search.set_hexpand(true);
        content.append(&search);

        let list = ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        let scroller = ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        content.append(&scroller);

        let clear = Button::with_label(&self.inner.translator.text("history.clear_button"));
        clear.set_halign(gtk::Align::End);
        clear.add_css_class("destructive-action");
        content.append(&clear);
        window.set_child(Some(&content));

        let weak_ui = Rc::downgrade(&self.inner);
        search.connect_search_changed(move |search| {
            if let Some(inner) = weak_ui.upgrade() {
                let ui = AppUi { inner };
                *ui.inner.search_query.borrow_mut() = search.text().to_string();
                ui.rebuild_list();
            }
        });
        let weak_ui = Rc::downgrade(&self.inner);
        let weak_window = window.downgrade();
        clear.connect_clicked(move |_| {
            if let Some(inner) = weak_ui.upgrade() {
                AppUi { inner }.confirm_clear(weak_window.upgrade().as_ref());
            }
        });

        let weak_window = window.downgrade();
        window.connect_close_request(move |window| {
            window.set_visible(false);
            let _ = &weak_window;
            gtk::glib::Propagation::Stop
        });

        *self.inner.window.borrow_mut() = Some(window.clone());
        *self.inner.list.borrow_mut() = Some(list);
        self.rebuild_list();
        window.present();
    }

    fn show_settings(&self) {
        if let Some(window) = self.inner.settings_window.borrow().as_ref() {
            window.present();
            return;
        }

        let window = ApplicationWindow::builder()
            .application(&self.inner.app)
            .title(self.inner.translator.text("settings.window_title"))
            .default_width(480)
            .default_height(280)
            .build();
        let header = HeaderBar::new();
        let header_title = Label::new(Some(&self.inner.translator.text("settings.window_title")));
        header.set_title_widget(Some(&header_title));
        window.set_titlebar(Some(&header));

        let content = GtkBox::new(Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);

        let title = Label::new(Some(&self.inner.translator.text("settings.heading")));
        title.set_xalign(0.0);
        title.add_css_class("title-2");
        content.append(&title);

        let primary = Switch::new();
        primary.set_valign(gtk::Align::Center);
        primary.set_sensitive(self.inner.clipboard.primary_supported());
        content.append(&setting_row(
            &self.inner.translator.text("settings.primary"),
            &primary,
        ));

        let max_history = SpinButton::with_range(1.0, 1000.0, 1.0);
        max_history.set_numeric(true);
        content.append(&setting_row(
            &self.inner.translator.text("settings.max_history"),
            &max_history,
        ));

        let max_menu = SpinButton::with_range(1.0, 1000.0, 1.0);
        max_menu.set_numeric(true);
        content.append(&setting_row(
            &self.inner.translator.text("settings.max_menu"),
            &max_menu,
        ));

        let hint = Label::new(Some(&self.inner.translator.text("settings.immediate_hint")));
        hint.set_xalign(0.0);
        hint.add_css_class("dim-label");
        content.append(&hint);
        window.set_child(Some(&content));

        let controls = SettingsControls {
            primary,
            max_history,
            max_menu,
        };
        self.apply_controls_from_state(&controls);
        *self.inner.settings_controls.borrow_mut() = Some(controls.clone());

        let weak_ui = Rc::downgrade(&self.inner);
        controls.primary.connect_active_notify(move |_| {
            if let Some(inner) = weak_ui.upgrade() {
                AppUi { inner }.save_settings_from_controls();
            }
        });
        let weak_ui = Rc::downgrade(&self.inner);
        controls.max_history.connect_value_changed(move |_| {
            if let Some(inner) = weak_ui.upgrade() {
                AppUi { inner }.save_settings_from_controls();
            }
        });
        let weak_ui = Rc::downgrade(&self.inner);
        controls.max_menu.connect_value_changed(move |_| {
            if let Some(inner) = weak_ui.upgrade() {
                AppUi { inner }.save_settings_from_controls();
            }
        });

        window.connect_close_request(|window| {
            window.set_visible(false);
            gtk::glib::Propagation::Stop
        });
        *self.inner.settings_window.borrow_mut() = Some(window.clone());
        window.present();
    }

    fn activate(&self, target: TrayTarget) {
        let payload = {
            let state = self.inner.state.borrow();
            match target {
                TrayTarget::Primary => state
                    .primary
                    .as_ref()
                    .map(|entry| (false, entry.id, entry.payload.clone())),
                TrayTarget::Regular(id) => state
                    .regular
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| (true, entry.id, entry.payload.clone())),
            }
        };

        let Some((is_regular, id, payload)) = payload else {
            warn!("tray requested an unknown history entry");
            return;
        };

        if let Err(error) = self.inner.clipboard.restore(&payload) {
            warn!(%error, "failed to restore clipboard payload");
            self.show_error(&error.to_string());
            return;
        }

        if is_regular {
            self.spawn_storage_operation(StorageOperation::Touch(id));
        }
    }

    fn request_clear_regular(&self) {
        self.spawn_storage_operation(StorageOperation::ClearRegular);
    }

    fn confirm_clear(&self, parent: Option<&ApplicationWindow>) {
        let dialog = AlertDialog::builder()
            .message(self.inner.translator.text("history.clear_confirm.message"))
            .detail(self.inner.translator.text("history.clear_confirm.detail"))
            .buttons([
                self.inner.translator.text("common.cancel").as_str(),
                self.inner.translator.text("common.clear").as_str(),
            ])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let weak_ui = Rc::downgrade(&self.inner);
        dialog.choose(parent, None::<&gtk::gio::Cancellable>, move |result| {
            if matches!(result, Ok(1))
                && let Some(inner) = weak_ui.upgrade()
            {
                AppUi { inner }.request_clear_regular();
            }
        });
    }

    fn save_settings_from_controls(&self) {
        if self.inner.settings_loading.get() {
            return;
        }
        let Some(controls) = self.inner.settings_controls.borrow().clone() else {
            return;
        };

        let max_history = controls.max_history.value_as_int().max(1) as usize;
        let mut max_menu = controls.max_menu.value_as_int().max(1) as usize;
        max_menu = max_menu.min(max_history);
        if controls.max_menu.value_as_int() != max_menu as i32 {
            self.inner.settings_loading.set(true);
            controls.max_menu.set_value(max_menu as f64);
            self.inner.settings_loading.set(false);
        }

        let settings = normalize_settings(controls.primary.is_active(), max_history, max_menu);
        if self.inner.state.borrow().settings == settings {
            return;
        }
        self.spawn_storage_operation(StorageOperation::UpdateSettings(settings));
    }

    fn sync_settings_controls(&self) {
        let Some(controls) = self.inner.settings_controls.borrow().clone() else {
            return;
        };
        self.apply_controls_from_state(&controls);
    }

    fn apply_controls_from_state(&self, controls: &SettingsControls) {
        let settings = self.inner.state.borrow().settings.clone();
        self.inner.settings_loading.set(true);
        controls.primary.set_active(settings.primary_enabled);
        controls
            .max_history
            .set_value(settings.max_history_entries as f64);
        controls
            .max_menu
            .set_value(settings.max_menu_entries as f64);
        self.inner.settings_loading.set(false);
    }

    fn spawn_storage_operation(&self, operation: StorageOperation) {
        let storage = self.inner.storage.clone();
        let clipboard = self.inner.clipboard.clone();
        let events = self.inner.storage_events.clone();
        self.inner.tokio_handle.spawn(async move {
            if matches!(operation, StorageOperation::ClearRegular) {
                match storage.clear_regular().await {
                    Ok(snapshot) => {
                        if let Err(error) = clipboard.clear_regular() {
                            let _ = events.send(StorageEvent::Error(error.to_string()));
                        }
                        let _ = events.send(StorageEvent::Snapshot(snapshot));
                    }
                    Err(error) => {
                        let _ = events.send(StorageEvent::Error(error.to_string()));
                    }
                }
                return;
            }

            let result = match operation {
                StorageOperation::Upsert(source, payload) => {
                    storage.upsert_capture(source, payload).await
                }
                StorageOperation::Touch(id) => storage.touch(id).await,
                StorageOperation::Delete(id) => storage.delete(id).await,
                StorageOperation::ClearRegular => unreachable!(),
                StorageOperation::UpdateSettings(settings) => {
                    storage.update_settings(settings).await
                }
            };
            let event = match result {
                Ok(snapshot) => StorageEvent::Snapshot(snapshot),
                Err(error) => StorageEvent::Error(error.to_string()),
            };
            let _ = events.send(event);
        });
    }

    fn publish(&self) {
        let snapshot = {
            let state = self.inner.state.borrow();
            TraySnapshot {
                primary: state.primary.as_ref().map(|entry| TrayItem {
                    id: entry.id,
                    preview: menu_preview(&entry.payload.plain_text),
                }),
                entries: state
                    .regular
                    .iter()
                    .take(state.settings.max_menu_entries)
                    .map(|entry| TrayItem {
                        id: entry.id,
                        preview: menu_preview(&entry.payload.plain_text),
                    })
                    .collect(),
            }
        };
        *self
            .inner
            .tray_snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot;
        let tray_handle = self.inner.tray_handle.clone();
        self.inner.tokio_handle.spawn(async move {
            let _ = tray_handle.update(|_| {}).await;
        });
        self.rebuild_list();
    }

    fn rebuild_list(&self) {
        let Some(list) = self.inner.list.borrow().as_ref().cloned() else {
            return;
        };
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }

        let query = self.inner.search_query.borrow().to_lowercase();
        let entries = {
            let state = self.inner.state.borrow();
            state
                .regular
                .iter()
                .filter(|entry| matches_search(&entry.payload.plain_text, &query))
                .map(|entry| (entry.id, entry.payload.plain_text.clone()))
                .collect::<Vec<_>>()
        };

        if entries.is_empty() {
            let text = if query.is_empty() {
                self.inner.translator.text("history.empty")
            } else {
                self.inner.translator.text("history.no_matches")
            };
            let label = Label::new(Some(&text));
            label.set_margin_top(24);
            label.set_margin_bottom(24);
            list.append(&label);
            return;
        }

        for (id, text) in entries {
            let row = ListBoxRow::new();
            let row_content = GtkBox::new(Orientation::Horizontal, 6);

            let content = Button::with_label(&text);
            content.set_has_frame(false);
            content.set_hexpand(true);
            content.set_halign(gtk::Align::Fill);
            content.set_widget_name("history-entry");
            let weak_ui = Rc::downgrade(&self.inner);
            content.connect_clicked(move |_| {
                if let Some(inner) = weak_ui.upgrade() {
                    AppUi { inner }.activate(TrayTarget::Regular(id));
                }
            });

            let delete = Button::from_icon_name("window-close-symbolic");
            delete.set_tooltip_text(Some(&self.inner.translator.text("history.delete_tooltip")));
            delete.add_css_class("flat");
            delete.set_opacity(0.0);
            delete.set_can_target(false);
            let weak_ui = Rc::downgrade(&self.inner);
            delete.connect_clicked(move |_| {
                if let Some(inner) = weak_ui.upgrade() {
                    AppUi { inner }.spawn_storage_operation(StorageOperation::Delete(id));
                }
            });

            let motion = EventControllerMotion::new();
            let delete_on_enter = delete.clone();
            motion.connect_enter(move |_, _, _| {
                delete_on_enter.set_opacity(1.0);
                delete_on_enter.set_can_target(true);
            });
            let delete_on_leave = delete.clone();
            motion.connect_leave(move |_| {
                delete_on_leave.set_opacity(0.0);
                delete_on_leave.set_can_target(false);
            });
            row_content.add_controller(motion);

            row_content.append(&content);
            row_content.append(&delete);
            row.set_child(Some(&row_content));
            list.append(&row);
        }
    }

    fn show_error(&self, detail: &str) {
        let dialog = AlertDialog::builder()
            .message(self.inner.translator.text("error.title"))
            .detail(detail)
            .buttons([self.inner.translator.text("common.ok").as_str()])
            .default_button(0)
            .build();
        dialog.choose(
            None::<&ApplicationWindow>,
            None::<&gtk::gio::Cancellable>,
            |_| {},
        );
    }
}

fn setting_row(label: &str, control: &impl IsA<gtk::Widget>) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 12);
    let label = Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    row.append(&label);
    row.append(control);
    row
}

fn matches_search(text: &str, lowercase_query: &str) -> bool {
    lowercase_query.is_empty() || text.to_lowercase().contains(lowercase_query)
}

fn normalize_settings(
    primary_enabled: bool,
    max_history_entries: usize,
    max_menu_entries: usize,
) -> Settings {
    let max_history_entries = max_history_entries.max(1);
    Settings {
        primary_enabled,
        max_history_entries,
        max_menu_entries: max_menu_entries.max(1).min(max_history_entries),
    }
}

impl Drop for UiInner {
    fn drop(&mut self) {
        if let Some(window) = self.window.get_mut().take() {
            window.destroy();
        }
        if let Some(window) = self.settings_window.get_mut().take() {
            window.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_is_case_insensitive() {
        assert!(matches_search("Hello Clipman", "hello"));
        assert!(matches_search("Hello Clipman", "clip"));
        assert!(!matches_search("Hello Clipman", "missing"));
        assert!(matches_search("anything", ""));
    }

    #[test]
    fn menu_limit_is_clamped_to_history_limit() {
        let settings = normalize_settings(false, 5, 9);
        assert_eq!(settings.max_history_entries, 5);
        assert_eq!(settings.max_menu_entries, 5);
        assert!(!settings.primary_enabled);
    }
}

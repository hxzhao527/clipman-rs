use std::sync::{Arc, Mutex};

use anyhow::Result;
use ksni::{Category, MenuItem, Tray, menu::StandardItem};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedSender;

use crate::i18n::Translator;

#[derive(Clone, Debug, Default)]
pub struct TrayItem {
    pub id: u64,
    pub preview: String,
}

#[derive(Clone, Debug, Default)]
pub struct TraySnapshot {
    pub primary: Option<TrayItem>,
    pub entries: Vec<TrayItem>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayTarget {
    Primary,
    Regular(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayCommand {
    Activate(TrayTarget),
    ShowAll,
    Clear,
    Settings,
    Exit,
}

pub struct ClipmanTray {
    snapshot: Arc<Mutex<TraySnapshot>>,
    command_tx: UnboundedSender<TrayCommand>,
    translator: Translator,
}

impl ClipmanTray {
    pub fn new(
        snapshot: Arc<Mutex<TraySnapshot>>,
        command_tx: UnboundedSender<TrayCommand>,
        translator: Translator,
    ) -> Self {
        Self {
            snapshot,
            command_tx,
            translator,
        }
    }
}

impl Tray for ClipmanTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "clipman-rs".to_owned()
    }

    fn title(&self) -> String {
        "clipman-rs".to_owned()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn icon_name(&self) -> String {
        "edit-paste".to_owned()
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut menu = Vec::new();

        if let Some(primary) = snapshot.primary {
            menu.push(
                StandardItem {
                    label: format!(
                        "{}: {}",
                        self.translator.text("menu.primary"),
                        primary.preview
                    ),
                    activate: action_callback(
                        self.command_tx.clone(),
                        TrayCommand::Activate(TrayTarget::Primary),
                    ),
                    ..Default::default()
                }
                .into(),
            );
            menu.push(MenuItem::Separator);
        }

        let has_entries = !snapshot.entries.is_empty();
        for entry in snapshot.entries {
            menu.push(
                StandardItem {
                    label: entry.preview,
                    activate: action_callback(
                        self.command_tx.clone(),
                        TrayCommand::Activate(TrayTarget::Regular(entry.id)),
                    ),
                    ..Default::default()
                }
                .into(),
            );
        }
        if !has_entries {
            menu.push(
                StandardItem {
                    label: self.translator.text("menu.empty"),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        }

        menu.push(MenuItem::Separator);
        menu.push(
            StandardItem {
                label: self.translator.text("menu.show_all"),
                activate: action_callback(self.command_tx.clone(), TrayCommand::ShowAll),
                ..Default::default()
            }
            .into(),
        );
        menu.push(
            StandardItem {
                label: self.translator.text("menu.clear_history"),
                activate: action_callback(self.command_tx.clone(), TrayCommand::Clear),
                ..Default::default()
            }
            .into(),
        );
        menu.push(
            StandardItem {
                label: self.translator.text("menu.settings"),
                activate: action_callback(self.command_tx.clone(), TrayCommand::Settings),
                ..Default::default()
            }
            .into(),
        );
        menu.push(
            StandardItem {
                label: self.translator.text("menu.exit"),
                activate: action_callback(self.command_tx.clone(), TrayCommand::Exit),
                ..Default::default()
            }
            .into(),
        );

        menu
    }
}

fn action_callback(
    command_tx: UnboundedSender<TrayCommand>,
    command: TrayCommand,
) -> Box<dyn Fn(&mut ClipmanTray) + Send> {
    Box::new(move |_| {
        let _ = command_tx.send(command);
    })
}

pub fn spawn_tray(
    runtime: &Runtime,
    snapshot: Arc<Mutex<TraySnapshot>>,
    command_tx: UnboundedSender<TrayCommand>,
    translator: Translator,
) -> Result<ksni::Handle<ClipmanTray>> {
    use ksni::TrayMethods;

    let tray = ClipmanTray::new(snapshot, command_tx, translator);
    runtime
        .block_on(tray.assume_sni_available(true).spawn())
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tray_for(snapshot: TraySnapshot) -> ClipmanTray {
        let (command_tx, _command_rx) = tokio::sync::mpsc::unbounded_channel();
        ClipmanTray::new(
            Arc::new(Mutex::new(snapshot)),
            command_tx,
            Translator::for_locale("zh_CN"),
        )
    }

    #[test]
    fn empty_history_has_disabled_placeholder() {
        let menu = tray_for(TraySnapshot::default()).menu();
        let MenuItem::Standard(item) = &menu[0] else {
            panic!("expected standard menu item");
        };
        assert_eq!(item.label, "暂无记录");
        assert!(!item.enabled);
    }

    #[test]
    fn primary_with_empty_history_still_has_placeholder() {
        let menu = tray_for(TraySnapshot {
            primary: Some(TrayItem {
                id: 0,
                preview: "primary".to_owned(),
            }),
            entries: Vec::new(),
        })
        .menu();
        let MenuItem::Standard(item) = &menu[2] else {
            panic!("expected standard menu item");
        };
        assert_eq!(item.label, "暂无记录");
        assert!(!item.enabled);
    }
}

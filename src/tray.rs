use std::sync::atomic::{AtomicBool, Ordering};

use async_channel::{Receiver, Sender};
use gettextrs::gettext;
use ksni::menu::StandardItem;
use ksni::{MenuItem, ToolTip, Tray};

use crate::config;

/// What the tray, on its own thread, asks of the application.
#[derive(Debug, Clone, Copy)]
pub enum TrayCommand {
    ShowWindow,
    Quit,
}

struct RillTray {
    tx: Sender<TrayCommand>,
}

impl Tray for RillTray {
    fn id(&self) -> String {
        config::APP_ID.into()
    }

    fn title(&self) -> String {
        "Rill".into()
    }

    fn icon_name(&self) -> String {
        // The symbolic icon takes the colour of the panel.
        format!("{}-symbolic", config::APP_ID)
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "Rill".into(),
            description: gettext("Download files over BitTorrent"),
            icon_name: config::APP_ID.into(),
            icon_pixmap: Vec::new(),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.try_send(TrayCommand::ShowWindow);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let tx_show = self.tx.clone();
        let tx_quit = self.tx.clone();
        vec![
            StandardItem {
                label: gettext("Show Rill"),
                activate: Box::new(move |_: &mut Self| {
                    let _ = tx_show.try_send(TrayCommand::ShowWindow);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: gettext("Quit"),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(move |_: &mut Self| {
                    let _ = tx_quit.try_send(TrayCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Set once the tray has registered with a StatusNotifier host. Closing the window hides
/// it only then: without a tray there would be no way back to it.
static AVAILABLE: AtomicBool = AtomicBool::new(false);

pub fn is_available() -> bool {
    AVAILABLE.load(Ordering::Acquire)
}

/// Starts the tray on its own thread and returns the commands it sends. Without a
/// StatusNotifier host the tray stays absent and nothing is ever sent.
pub fn spawn() -> Receiver<TrayCommand> {
    use ksni::blocking::TrayMethods;

    let (tx, rx) = async_channel::unbounded();
    // Registering waits on the session bus, which the window need not wait for.
    let registered = std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || match (RillTray { tx }).spawn() {
            Ok(handle) => {
                AVAILABLE.store(true, Ordering::Release);
                // The tray's service loop ends when its handle is dropped; it lives as
                // long as the process.
                std::mem::forget(handle);
            }
            Err(e) => log::warn!("System tray unavailable: {e}"),
        });
    if let Err(e) = registered {
        log::warn!("System tray unavailable: {e}");
    }
    rx
}

use async_channel::{Receiver, Sender};
use gettextrs::gettext;
use ksni::menu::StandardItem;
use ksni::{MenuItem, ToolTip, Tray};

/// Commands emitted by the tray (on its own thread) for the GTK main thread.
#[derive(Debug, Clone, Copy)]
pub enum TrayCommand {
    ShowWindow,
    QuitApp,
}

struct RillTray {
    tx: Sender<TrayCommand>,
}

impl Tray for RillTray {
    fn id(&self) -> String {
        "com.github.sachesi.rill".into()
    }

    fn title(&self) -> String {
        "Rill".into()
    }

    fn icon_name(&self) -> String {
        // Monochrome symbolic variant so the tray icon matches the panel
        // foreground colour instead of showing the full-colour app icon.
        "com.github.sachesi.rill-symbolic".into()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "Rill".into(),
            description: gettext("BitTorrent client"),
            icon_name: "com.github.sachesi.rill".into(),
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
                icon_name: "go-up-symbolic".into(),
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
                    let _ = tx_quit.try_send(TrayCommand::QuitApp);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Whether the tray registered with a StatusNotifier host. The window's
/// close-to-tray behaviour checks this: hiding without a tray would leave the
/// app running with no visible way back.
static TRAY_AVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_available() -> bool {
    TRAY_AVAILABLE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Spawn the system tray on its own thread. Returns a receiver to be drained on
/// the GTK main thread. If no StatusNotifier host is available the tray simply
/// stays hidden; the window can still be restored by relaunching the app.
pub fn spawn() -> Receiver<TrayCommand> {
    use ksni::blocking::TrayMethods;

    let (tx, rx) = async_channel::unbounded();
    match (RillTray { tx }).spawn() {
        // Keep the handle alive for the whole process so the tray's background
        // service loop is not torn down when the handle is dropped.
        Ok(handle) => {
            TRAY_AVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
            std::mem::forget(handle);
        }
        Err(e) => log::warn!("System tray unavailable: {}", e),
    }
    rx
}

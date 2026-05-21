use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use gtk::glib;
use glib::subclass::prelude::*;

use crate::engine::TorrentUiState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, glib::Enum)]
#[enum_type(name = "RillTorrentState")]
pub enum TorrentState {
    #[default]
    Downloading,
    Paused, 
    Completed,
    Error,
}

impl From<TorrentUiState> for TorrentState {
    fn from(state: TorrentUiState) -> Self {
        match state {
            TorrentUiState::Downloading => Self::Downloading,
            TorrentUiState::Paused => Self::Paused,
            TorrentUiState::Completed => Self::Completed,
            TorrentUiState::Error => Self::Error,
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct TorrentObject {
        pub info_hash: RefCell<String>,
        pub name: RefCell<String>,
        pub state: Cell<TorrentState>,
        pub downloaded: Cell<u64>,
        pub total: Cell<u64>,
        pub peers: Cell<u32>,
        pub speed_down: Cell<u64>,
        pub speed_up: Cell<u64>,
        pub output_dir: RefCell<String>,
        pub uri: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TorrentObject {
        const NAME: &'static str = "RillTorrentObject";
        type Type = super::TorrentObject;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for TorrentObject {}
}

glib::wrapper! {
    pub struct TorrentObject(ObjectSubclass<imp::TorrentObject>);
}

impl TorrentObject {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        info_hash: &str,
        name: &str,
        state: TorrentState,
        downloaded: u64,
        total: u64,
        peers: u32,
        speed_down: u64,
        speed_up: u64,
        output_dir: PathBuf,
        uri: &str,
    ) -> Self {
        let obj: Self = glib::Object::new();
        obj.set_info_hash(info_hash);
        obj.set_name(name);
        obj.set_state(state);
        obj.set_downloaded(downloaded);
        obj.set_total(total);
        obj.set_peers(peers);
        obj.set_speed_down(speed_down);
        obj.set_speed_up(speed_up);
        obj.set_output_dir(&output_dir.to_string_lossy());
        obj.set_uri(uri);
        obj
    }

    // Getters
    pub fn info_hash(&self) -> String {
        self.imp().info_hash.borrow().clone()
    }

    pub fn name(&self) -> String {
        self.imp().name.borrow().clone()
    }

    pub fn state(&self) -> TorrentState {
        self.imp().state.get()
    }

    pub fn downloaded(&self) -> u64 {
        self.imp().downloaded.get()
    }

    pub fn total(&self) -> u64 {
        self.imp().total.get()
    }

    pub fn peers(&self) -> u32 {
        self.imp().peers.get()
    }

    pub fn speed_down(&self) -> u64 {
        self.imp().speed_down.get()
    }

    pub fn speed_up(&self) -> u64 {
        self.imp().speed_up.get()
    }

    pub fn output_dir(&self) -> String {
        self.imp().output_dir.borrow().clone()
    }

    pub fn uri(&self) -> String {
        self.imp().uri.borrow().clone()
    }

    // Setters
    pub fn set_info_hash(&self, info_hash: &str) {
        self.imp().info_hash.replace(info_hash.to_string());
    }

    pub fn set_name(&self, name: &str) {
        self.imp().name.replace(name.to_string());
    }

    pub fn set_state(&self, state: TorrentState) {
        self.imp().state.set(state);
    }

    pub fn set_downloaded(&self, downloaded: u64) {
        self.imp().downloaded.set(downloaded);
    }

    pub fn set_total(&self, total: u64) {
        self.imp().total.set(total);
    }

    pub fn set_peers(&self, peers: u32) {
        self.imp().peers.set(peers);
    }

    pub fn set_speed_down(&self, speed_down: u64) {
        self.imp().speed_down.set(speed_down);
    }

    pub fn set_speed_up(&self, speed_up: u64) {
        self.imp().speed_up.set(speed_up);
    }

    pub fn set_output_dir(&self, output_dir: &str) {
        self.imp().output_dir.replace(output_dir.to_string());
    }

    pub fn set_uri(&self, uri: &str) {
        self.imp().uri.replace(uri.to_string());
    }

    /// Get progress as a percentage (0.0 to 1.0)
    pub fn progress(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            0.0
        } else {
            self.downloaded() as f64 / total as f64
        }
    }

    /// Get human-readable size string for downloaded/total
    pub fn size_text(&self) -> String {
        format!(
            "{} of {}",
            format_size(self.downloaded()),
            format_size(self.total())
        )
    }

    /// Get human-readable speed string
    pub fn speed_text(&self) -> String {
        if self.speed_down() == 0 {
            String::new()
        } else {
            format!("↓ {}/s", format_size(self.speed_down()))
        }
    }

    /// Get status text combining speed and peers
    pub fn status_text(&self) -> String {
        let mut parts = Vec::new();
        
        if self.speed_down() > 0 {
            parts.push(format!("↓ {}/s", format_size(self.speed_down())));
        }
        
        if self.peers() > 0 {
            parts.push(format!("{} peers", self.peers()));
        }
        
        match self.state() {
            TorrentState::Downloading => {
                if parts.is_empty() {
                    "Connecting to peers...".to_string()
                } else {
                    parts.join(" • ")
                }
            }
            TorrentState::Paused => "Paused".to_string(),
            TorrentState::Completed => "Completed".to_string(),
            TorrentState::Error => "Error".to_string(),
        }
    }

    /// Get the output directory as PathBuf
    pub fn output_dir_path(&self) -> PathBuf {
        PathBuf::from(self.output_dir())
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;
    
    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }
    
    if unit_index == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}
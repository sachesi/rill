use std::collections::HashMap;
use std::cell::RefCell;
use std::rc::Rc;

use gtk::{gio, prelude::*};


use super::{TorrentObject};
use super::torrent_object::TorrentState;
use crate::engine::UiUpdate;

/// Manages the list of torrents and their organization into sections
#[derive(Debug, Clone)]
pub struct TorrentModel {
    /// All torrents in one store
    all_torrents: gio::ListStore,
    /// Filtered models for each section
    downloading: gtk::FilterListModel,
    paused: gtk::FilterListModel,
    completed: gtk::FilterListModel,
    /// Quick lookup by info_hash
    lookup: Rc<RefCell<HashMap<String, TorrentObject>>>,
}

impl TorrentModel {
    pub fn new() -> Self {
        let all_torrents = gio::ListStore::new::<TorrentObject>();
        let lookup = Rc::new(RefCell::new(HashMap::new()));

        // Create filters for each section
        let downloading_filter = gtk::CustomFilter::new(move |obj| {
            obj.downcast_ref::<TorrentObject>()
                .map(|t| t.state() == TorrentState::Downloading)
                .unwrap_or(false)
        });

        let paused_filter = gtk::CustomFilter::new(move |obj| {
            obj.downcast_ref::<TorrentObject>()
                .map(|t| t.state() == TorrentState::Paused)
                .unwrap_or(false)
        });

        let completed_filter = gtk::CustomFilter::new(move |obj| {
            obj.downcast_ref::<TorrentObject>()
                .map(|t| matches!(t.state(), TorrentState::Completed | TorrentState::Error))
                .unwrap_or(false)
        });

        let downloading = gtk::FilterListModel::new(Some(all_torrents.clone()), Some(downloading_filter));
        let paused = gtk::FilterListModel::new(Some(all_torrents.clone()), Some(paused_filter));
        let completed = gtk::FilterListModel::new(Some(all_torrents.clone()), Some(completed_filter));

        Self {
            all_torrents,
            downloading,
            paused,
            completed,
            lookup,
        }
    }

    /// Get the list model for the downloading section
    pub fn downloading_model(&self) -> &gtk::FilterListModel {
        &self.downloading
    }

    /// Get the list model for the paused section
    pub fn paused_model(&self) -> &gtk::FilterListModel {
        &self.paused
    }

    /// Get the list model for the completed section  
    pub fn completed_model(&self) -> &gtk::FilterListModel {
        &self.completed
    }

    /// Get count of items in each section
    pub fn counts(&self) -> (u32, u32, u32) {
        (
            self.downloading.n_items(),
            self.paused.n_items(),
            self.completed.n_items(),
        )
    }

    /// Check if there are any torrents at all
    pub fn is_empty(&self) -> bool {
        self.all_torrents.n_items() == 0
    }

    /// Add or update a torrent from a UI update
    pub fn update_torrent(&self, update: UiUpdate) {
        let mut lookup = self.lookup.borrow_mut();
        
        if let Some(existing) = lookup.get(&update.info_hash) {
            // Update existing torrent
            existing.set_name(&update.name);
            existing.set_state(TorrentState::from(update.state));
            existing.set_downloaded(update.downloaded);
            existing.set_total(update.total);
            existing.set_peers(update.peers as u32);
            existing.set_speed_down(update.speed_down);
            existing.set_speed_up(update.speed_up);
            existing.set_output_dir(&update.output_dir.to_string_lossy());
            existing.set_uri(&update.uri);
            
            // Notify filters that this object changed
            self.all_torrents.items_changed(0, 0, 0);
        } else {
            // Create new torrent
            let torrent = TorrentObject::new(
                &update.info_hash,
                &update.name,
                TorrentState::from(update.state),
                update.downloaded,
                update.total,
                update.peers as u32,
                update.speed_down,
                update.speed_up,
                update.output_dir,
                &update.uri,
            );
            
            lookup.insert(update.info_hash.clone(), torrent.clone());
            self.all_torrents.append(&torrent);
        }
    }

    /// Remove a torrent by info_hash
    pub fn remove_torrent(&self, info_hash: &str) -> bool {
        let mut lookup = self.lookup.borrow_mut();
        
        if let Some(torrent) = lookup.remove(info_hash) {
            // Find and remove from the main store
            for i in 0..self.all_torrents.n_items() {
                if let Some(item) = self.all_torrents.item(i) {
                    if let Some(t) = item.downcast_ref::<TorrentObject>() {
                        if t.info_hash() == info_hash {
                            self.all_torrents.remove(i);
                            return true;
                        }
                    }
                }
            }
        }
        
        false
    }

    /// Get a torrent by info_hash
    pub fn get_torrent(&self, info_hash: &str) -> Option<TorrentObject> {
        self.lookup.borrow().get(info_hash).cloned()
    }

    /// Get all torrents
    pub fn all_torrents(&self) -> Vec<TorrentObject> {
        self.lookup.borrow().values().cloned().collect()
    }

    /// Clear all torrents
    pub fn clear(&self) {
        self.lookup.borrow_mut().clear();
        self.all_torrents.remove_all();
    }
}
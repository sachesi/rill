use std::sync::atomic::Ordering;

use adw::prelude::*;
use gtk::glib;

use crate::logging;
use crate::storage::Storage;

pub fn show_preferences(parent: &crate::application::RillWindow, storage: Storage) {
    let prefs = adw::PreferencesWindow::builder()
        .title("Preferences")
        .transient_for(parent)
        .modal(true)
        .build();

    // Load current settings
    let settings = storage.load_settings();
    let current_folder = settings.download_folder_path();

    // General page
    let general_page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    let download_group = adw::PreferencesGroup::builder()
        .title("Downloads")
        .build();

    let download_row = adw::ActionRow::builder()
        .title("Download Folder")
        .subtitle(current_folder.to_string_lossy().as_ref())
        .build();

    let folder_btn = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Choose Folder")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();

    // Wire folder picker
    let storage_clone = storage.clone();
    let download_row_weak = download_row.downgrade();
    let parent_clone1 = parent.clone();
    folder_btn.connect_clicked(glib::clone!(
        #[weak] prefs,
        move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Choose Download Folder")
                .modal(true)
                .build();

            let storage_clone2 = storage_clone.clone();
            let download_row_weak2 = download_row_weak.clone();
            let parent_clone1 = parent_clone1.clone();
            dialog.select_folder(
                Some(&prefs),
                gtk::gio::Cancellable::NONE,
                move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        // Update subtitle
                        if let Some(row) = download_row_weak2.upgrade() {
                            row.set_subtitle(&path.to_string_lossy());
                        }
                        
                        // Save to storage
                        let mut settings = storage_clone2.load_settings();
                        settings.download_folder = path.to_string_lossy().to_string();
                        if let Err(e) = storage_clone2.save_settings(&settings) {
                            log::warn!("Failed to save settings: {}", e);
                            parent_clone1.show_toast(&format!("Failed to save download folder: {}", e));
                        } else {
                            log::info!("Download folder updated: {}", path.display());
                        }
                    }
                },
            );
        }
    ));

    download_row.add_suffix(&folder_btn);
    download_group.add(&download_row);
    general_page.add(&download_group);
    prefs.add(&general_page);

    // Logging page
    let logging_page = adw::PreferencesPage::builder()
        .title("Logging")
        .icon_name("dialog-information-symbolic")
        .build();

    let level_group = adw::PreferencesGroup::builder()
        .title("Log Level")
        .description("Controls how much detail is printed to the terminal")
        .build();

    let log_model = gtk::StringList::new(&["Error", "Warn", "Info", "Debug", "Trace"]);
    let log_level_row = adw::ComboRow::builder()
        .title("Log Level")
        .model(&log_model)
        .selected(match settings.log_level.as_str() {
            "error" => 0,
            "warn" => 1,
            "info" => 2,
            "debug" => 3,
            "trace" => 4,
            _ => 2,
        })
        .build();
    level_group.add(&log_level_row);

    let ops_switch = adw::SwitchRow::builder()
        .title("Log Torrent Operations")
        .subtitle("Log detailed start/stop/pause/resume events")
        .active(settings.log_torrent_ops)
        .build();
    level_group.add(&ops_switch);
    logging_page.add(&level_group);

    // Wire log level changes
    let storage1 = storage.clone();
    let log_model2 = log_model.clone();
    let parent_clone2 = parent.clone();
    log_level_row.connect_selected_notify(move |row| {
        let idx = row.selected();
        let level_str = log_model2
            .string(idx)
            .map(|s| s.to_lowercase().to_string())
            .unwrap_or_else(|| "info".to_string());

        let mut settings = storage1.load_settings();
        settings.log_level = level_str;
        if let Err(e) = storage1.save_settings(&settings) {
            log::warn!("Failed to save log level: {}", e);
            parent_clone2.show_toast(&format!("Failed to save log level: {}", e));
        }
        logging::apply_settings(&settings);
    });

    // Wire torrent ops toggle
    let storage2 = storage.clone();
    let parent_clone3 = parent.clone();
    ops_switch.connect_active_notify(move |sw| {
        let mut settings = storage2.load_settings();
        settings.log_torrent_ops = sw.is_active();
        if let Err(e) = storage2.save_settings(&settings) {
            log::warn!("Failed to save log torrent ops: {}", e);
            parent_clone3.show_toast(&format!("Failed to save log torrent ops: {}", e));
        }
        logging::TORRENT_OPS_ENABLED.store(sw.is_active(), Ordering::Relaxed);
    });

    // Network page
    let network_page = adw::PreferencesPage::builder()
        .title("Network")
        .icon_name("network-wired-symbolic")
        .build();

    let limits_group = adw::PreferencesGroup::builder()
        .title("Bandwidth Limits")
        .build();

    let dl_limit_adj = gtk::Adjustment::new(
        settings.global_download_limit as f64,
        0.0,
        1000000.0,
        10.0,
        100.0,
        0.0,
    );
    let dl_limit_row = adw::SpinRow::builder()
        .title("Global Download Limit (KiB/s)")
        .subtitle("0 means unlimited")
        .adjustment(&dl_limit_adj)
        .digits(0)
        .build();

    let ul_limit_adj = gtk::Adjustment::new(
        settings.global_upload_limit as f64,
        0.0,
        1000000.0,
        10.0,
        100.0,
        0.0,
    );
    let ul_limit_row = adw::SpinRow::builder()
        .title("Global Upload Limit (KiB/s)")
        .subtitle("0 means unlimited")
        .adjustment(&ul_limit_adj)
        .digits(0)
        .build();

    limits_group.add(&dl_limit_row);
    limits_group.add(&ul_limit_row);
    network_page.add(&limits_group);

    let conn_group = adw::PreferencesGroup::builder()
        .title("Connection")
        .build();

    let port_adj = gtk::Adjustment::new(
        settings.pwp_port as f64,
        0.0,
        65535.0,
        1.0,
        10.0,
        0.0,
    );
    let port_row = adw::SpinRow::builder()
        .title("Peer Listening Port")
        .subtitle("0 means random port")
        .adjustment(&port_adj)
        .digits(0)
        .build();

    conn_group.add(&port_row);
    network_page.add(&conn_group);

    let queue_group = adw::PreferencesGroup::builder()
        .title("Transfer Queue")
        .build();

    let max_dl_adj = gtk::Adjustment::new(
        settings.max_active_downloads as f64,
        1.0,
        50.0,
        1.0,
        5.0,
        0.0,
    );
    let max_dl_row = adw::SpinRow::builder()
        .title("Max Active Downloads")
        .adjustment(&max_dl_adj)
        .digits(0)
        .build();

    let max_ul_adj = gtk::Adjustment::new(
        settings.max_active_uploads as f64,
        1.0,
        50.0,
        1.0,
        5.0,
        0.0,
    );
    let max_ul_row = adw::SpinRow::builder()
        .title("Max Active Uploads")
        .adjustment(&max_ul_adj)
        .digits(0)
        .build();

    let ratio_adj = gtk::Adjustment::new(
        settings.seeding_ratio_limit,
        0.0,
        100.0,
        0.1,
        1.0,
        0.0,
    );
    let ratio_row = adw::SpinRow::builder()
        .title("Seeding Ratio Limit")
        .adjustment(&ratio_adj)
        .digits(1)
        .build();

    queue_group.add(&max_dl_row);
    queue_group.add(&max_ul_row);
    queue_group.add(&ratio_row);
    network_page.add(&queue_group);

    prefs.add(&network_page);
    prefs.add(&logging_page);

    // Wire limits changes
    let storage_port = storage.clone();
    let parent_clone_port = parent.clone();
    port_adj.connect_value_changed(move |adj| {
        let mut settings = storage_port.load_settings();
        settings.pwp_port = adj.value() as u16;
        if let Err(e) = storage_port.save_settings(&settings) {
            log::warn!("Failed to save peer listening port: {}", e);
            parent_clone_port.show_toast(&format!("Failed to save port: {}", e));
        }
    });

    let storage_dl = storage.clone();
    let parent_clone4 = parent.clone();
    dl_limit_adj.connect_value_changed(move |adj| {
        let mut settings = storage_dl.load_settings();
        settings.global_download_limit = adj.value() as i32;
        if let Err(e) = storage_dl.save_settings(&settings) {
            log::warn!("Failed to save global download limit: {}", e);
            parent_clone4.show_toast(&format!("Failed to save download limit: {}", e));
        }
    });

    let storage_ul = storage.clone();
    let parent_clone5 = parent.clone();
    ul_limit_adj.connect_value_changed(move |adj| {
        let mut settings = storage_ul.load_settings();
        settings.global_upload_limit = adj.value() as i32;
        if let Err(e) = storage_ul.save_settings(&settings) {
            log::warn!("Failed to save global upload limit: {}", e);
            parent_clone5.show_toast(&format!("Failed to save upload limit: {}", e));
        }
    });

    let storage_max_dl = storage.clone();
    let parent_clone6 = parent.clone();
    max_dl_adj.connect_value_changed(move |adj| {
        let mut settings = storage_max_dl.load_settings();
        settings.max_active_downloads = adj.value() as i32;
        if let Err(e) = storage_max_dl.save_settings(&settings) {
            log::warn!("Failed to save max active downloads: {}", e);
            parent_clone6.show_toast(&format!("Failed to save max downloads: {}", e));
        }
    });

    let storage_max_ul = storage.clone();
    let parent_clone7 = parent.clone();
    max_ul_adj.connect_value_changed(move |adj| {
        let mut settings = storage_max_ul.load_settings();
        settings.max_active_uploads = adj.value() as i32;
        if let Err(e) = storage_max_ul.save_settings(&settings) {
            log::warn!("Failed to save max active uploads: {}", e);
            parent_clone7.show_toast(&format!("Failed to save max uploads: {}", e));
        }
    });

    let storage_ratio = storage;
    let parent_clone8 = parent.clone();
    ratio_adj.connect_value_changed(move |adj| {
        let mut settings = storage_ratio.load_settings();
        settings.seeding_ratio_limit = adj.value();
        if let Err(e) = storage_ratio.save_settings(&settings) {
            log::warn!("Failed to save seeding ratio limit: {}", e);
            parent_clone8.show_toast(&format!("Failed to save ratio limit: {}", e));
        }
    });

    prefs.present();
}
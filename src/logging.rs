use std::sync::atomic::{AtomicBool, Ordering};

use log::LevelFilter;

use crate::storage::AppSettings;

pub static TORRENT_OPS_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn apply_settings(settings: &AppSettings) {
    let level = level_from_str(&settings.log_level).unwrap_or(LevelFilter::Info);
    log::set_max_level(level);
    TORRENT_OPS_ENABLED.store(settings.log_torrent_ops, Ordering::Relaxed);
}

pub fn level_from_str(s: &str) -> Option<LevelFilter> {
    match s {
        "error" => Some(LevelFilter::Error),
        "warn" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

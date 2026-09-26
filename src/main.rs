mod application;
mod config;
mod dialogs;
mod engine;
mod listener;
mod logging;
mod storage;
#[cfg(test)]
mod test_support;
mod torrent_paths;
mod torrent_row;
mod torrents;
mod tray;
mod util;
mod window;

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use mtorrent as mt;
use mtorrent::utils::re_exports::mtorrent_utils::peer_id::PeerId;

use crate::application::{RillApplication, Session};
use crate::engine::TorrentEngine;
use crate::storage::Storage;

fn main() -> glib::ExitCode {
    // SAFETY: the first thing the program does; no thread has been started.
    unsafe { init_locale() };

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("trace"))
        .filter_module("mtorrent::app::dht", log::LevelFilter::Warn)
        .filter_module("mtorrent::app::main", log::LevelFilter::Warn)
        .filter_module("mtorrent_base::utp", log::LevelFilter::Error)
        // mtorrent logs routine peer churn (reset, interrupted, bad ack) as errors.
        .filter_module("mtorrent_base::utp::handle", log::LevelFilter::Off)
        .filter_module("mtorrent_base::utp::udp", log::LevelFilter::Off)
        .init();

    raise_open_file_limit();

    // The peer and storage runtimes run on detached threads; a panic there would
    // otherwise only reach stderr, not the log.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("Thread panicked: {info}");
        default_hook(info);
    }));

    gio::resources_register_include!("rill.gresource").expect("register resources");
    glib::set_application_name("Rill");

    // Registering claims the application id on the session bus before anything heavy
    // happens. A second launch is then remote: run() hands its magnet link or file to
    // the running instance and returns, without opening the database or the DHT port.
    let app = RillApplication::new();
    if let Err(e) = app.register(gio::Cancellable::NONE) {
        log::error!("Failed to register the application: {e}");
    }
    if app.is_remote() {
        log::info!("Rill is already running; passing this launch to it");
        return app.run();
    }

    match start_session() {
        Ok(session) => app.set_session(session),
        Err(e) => {
            log::error!("{e}");
            eprintln!("rill: {e}");
            return glib::ExitCode::FAILURE;
        }
    }
    // Exit without unwinding: dropping the runtimes would wait on transfers the
    // shutdown handler has already stopped.
    std::process::exit(app.run().into())
}

/// Opens the database and starts the engine: the runtimes mtorrent needs, the DHT node,
/// and the torrents saved from the last session.
fn start_session() -> Result<Session, String> {
    let data_dir = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .ok_or("No data directory; is HOME set?")?
        .join("rill");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("Could not create {}: {e}", data_dir.display()))?;
    log::info!("Data directory: {}", data_dir.display());

    let db_path = data_dir.join("torrents.db");
    let storage = Storage::open(db_path.clone())
        .map_err(|e| format!("Could not open the database {}: {e}", db_path.display()))?;
    let settings = storage.load_settings();
    logging::apply_settings(&settings);

    // Peer connections want a current-thread runtime of their own (they use spawn_local).
    // The builder is Send, the runtime is not, so it is built on the thread that drives it.
    let pwp_handle = spawn_local_runtime("pwp-runtime")?;
    let storage_runtime =
        storage_runtime().map_err(|e| format!("Could not start storage-runtime: {e}"))?;

    let (dht_worker, dht_cmds) = mt::app::dht::launch_dht_node_runtime(mt::app::dht::Config {
        local_port: dht_port(),
        max_concurrent_queries: None,
        config_dir: data_dir.clone(),
        use_upnp: false,
        bootstrap_nodes_override: None,
        bind_interface: None,
        query_timeout: None,
    })
    .map_err(|e| format!("Could not start the DHT node: {e}"))?;

    let engine = Rc::new(
        TorrentEngine::new(
            PeerId::generate_new(),
            data_dir,
            pwp_handle,
            storage_runtime.handle().clone(),
            dht_cmds,
            settings.pwp_port,
        )
        .map_err(|e| format!("Could not start the engine: {e}"))?,
    );

    let mut saved = storage.load_torrents().unwrap_or_else(|e| {
        log::warn!("Failed to load torrents: {e}");
        Vec::new()
    });
    rekey_legacy_records(&storage, &mut saved);
    log::info!("Loaded {} saved torrents", saved.len());

    Ok(Session {
        dht_worker: Some(dht_worker).into(),
        _storage_runtime: storage_runtime,
        engine,
        storage,
        saved: saved.into(),
    })
}

/// Most the soft limit on open files is raised to, whatever the hard limit: the kernel
/// allows no more by default anyway.
const OPEN_FILES_CEILING: libc::rlim_t = 1 << 20;

/// Raises the soft limit on open files as far as the hard limit allows. A desktop session
/// starts programs with a soft limit of 1024, and every peer connection is a file: a few
/// busy torrents reach it. Beyond it the torrents fail to connect, and so does the graphics
/// driver, which needs files of its own for each frame, reports itself out of memory, and
/// leaves the window waiting on a frame forever.
fn raise_open_file_limit() {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid rlimit for getrlimit to fill in.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        log::warn!(
            "Could not read the open file limit: {}",
            std::io::Error::last_os_error()
        );
        return;
    }
    let Some(wanted) = raised_soft_limit(&limit) else {
        return;
    };
    let previous = limit.rlim_cur;
    limit.rlim_cur = wanted;
    // SAFETY: `limit` is a valid rlimit, its soft value no higher than its hard one.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } == 0 {
        log::info!("Raised the open file limit from {previous} to {wanted}");
    } else {
        log::warn!(
            "Could not raise the open file limit from {previous}: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// The soft limit on open files to raise `limit` to, or `None` when it is as high as it goes.
fn raised_soft_limit(limit: &libc::rlimit) -> Option<libc::rlim_t> {
    let wanted = limit.rlim_max.min(OPEN_FILES_CEILING);
    (limit.rlim_cur < wanted).then_some(wanted)
}

/// The UDP port for the DHT node: the usual 6881, or the next free one when another client
/// holds it. mtorrent only logs a port it cannot bind and runs without the DHT.
fn dht_port() -> u16 {
    const USUAL: u16 = 6881;
    let port = free_udp_port(USUAL..=USUAL + 8);
    match port {
        USUAL => {}
        0 => log::warn!(
            "UDP ports {USUAL} to {} are in use; the DHT takes any free port",
            USUAL + 8
        ),
        port => log::warn!("UDP port {USUAL} is in use; the DHT takes port {port}"),
    }
    port
}

/// The first of `ports` that a UDP socket can bind, or 0, which leaves the choice to the
/// system.
fn free_udp_port(ports: impl IntoIterator<Item = u16>) -> u16 {
    ports
        .into_iter()
        .find(|&port| std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).is_ok())
        .unwrap_or(0)
}

/// Threads of the storage runtime: enough for the default three active downloads.
const STORAGE_THREADS: usize = 4;

/// The runtime torrents store their data on. mtorrent reads and writes each torrent's files
/// synchronously on it, so it has threads to spare: one torrent's disk must not hold up another.
fn storage_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(STORAGE_THREADS)
        .thread_name("storage-runtime")
        .build()
}

fn spawn_local_runtime(name: &str) -> Result<tokio::runtime::Handle, String> {
    let mut builder = tokio::runtime::Builder::new_current_thread();
    builder.enable_all();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || match builder.build_local(Default::default()) {
            Ok(rt) => {
                tx.send(Ok(rt.handle().clone())).ok();
                rt.block_on(std::future::pending::<()>());
            }
            Err(e) => {
                tx.send(Err(e.to_string())).ok();
            }
        })
        .map_err(|e| format!("Could not start {name}: {e}"))?;
    rx.recv()
        .map_err(|_| format!("{name} exited during startup"))?
        .map_err(|e| format!("{name}: {e}"))
}

/// Records saved before torrents were keyed by their info hash used a hash of the URI
/// text. Re-key them so the engine finds the same rows; a record whose info hash is
/// already taken keeps its old key.
///
/// Only a key made from the URI text is looked at: a record keyed by its info hash stays
/// so while its .torrent file is gone, and the files of the others are not read on every
/// start.
fn rekey_legacy_records(storage: &Storage, saved: &mut [storage::SavedTorrent]) {
    for torrent in saved {
        if torrent.info_hash != engine::hash_uri(&torrent.uri) {
            continue;
        }
        let Some(canonical) = engine::info_hash(&torrent.uri) else {
            continue;
        };
        match storage.migrate_torrent_hash(&torrent.info_hash, &canonical) {
            Ok(true) => {
                log::info!(
                    "Re-keyed torrent {} -> {} ({})",
                    torrent.info_hash,
                    canonical,
                    torrent.name
                );
                torrent.info_hash = canonical;
            }
            Ok(false) => log::warn!(
                "Torrent {} keeps its old id {}; its info hash is already in use",
                torrent.name,
                torrent.info_hash
            ),
            Err(e) => log::warn!("Failed to re-key {}: {e}", torrent.name),
        }
    }
}

/// Binds the text domain: the catalogues compiled into the build directory for a debug
/// build run from the source tree, the installed ones otherwise.
///
/// # Safety
///
/// Sets the locale, which reads the environment: call it before any thread is started.
unsafe fn init_locale() {
    use gettextrs::{
        LocaleCategory, bind_textdomain_codeset, bindtextdomain, setlocale, textdomain,
    };

    // SAFETY: the caller has started no thread yet.
    unsafe { setlocale(LocaleCategory::LcAll, "") };
    let dir = match option_env!("RILL_BUILD_LOCALEDIR") {
        Some(dir) if cfg!(debug_assertions) && std::path::Path::new(dir).is_dir() => dir,
        _ => config::LOCALEDIR,
    };
    bindtextdomain(config::GETTEXT_PACKAGE, PathBuf::from(dir)).ok();
    bind_textdomain_codeset(config::GETTEXT_PACKAGE, "UTF-8").ok();
    textdomain(config::GETTEXT_PACKAGE).ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_open_file_limit_is_raised_to_the_hard_one() {
        // Worked out rather than set: the limit is the whole process's, and the other tests
        // run alongside.
        let limit = |rlim_cur, rlim_max| libc::rlimit { rlim_cur, rlim_max };
        // A desktop session's soft limit, under the hard limit systemd sets or under none.
        assert_eq!(raised_soft_limit(&limit(1024, 524_288)), Some(524_288));
        assert_eq!(
            raised_soft_limit(&limit(1024, libc::RLIM_INFINITY)),
            Some(OPEN_FILES_CEILING)
        );
        assert_eq!(raised_soft_limit(&limit(4096, 4096)), None);
        assert_eq!(
            raised_soft_limit(&limit(OPEN_FILES_CEILING, libc::RLIM_INFINITY)),
            None
        );
    }

    #[test]
    fn dht_port_skips_ports_in_use() {
        let held = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let taken = held.local_addr().unwrap().port();
        let free = std::net::UdpSocket::bind("0.0.0.0:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        assert_eq!(free_udp_port([taken, free]), free);
        assert_eq!(free_udp_port([taken]), 0);
    }

    #[test]
    fn only_records_keyed_by_their_uri_are_rekeyed() {
        let dir = test_support::ScratchDir::new("rekey");
        let storage = Storage::open(dir.path().join("torrents.db")).unwrap();
        let hex = "0123456789abcdef0123456789abcdef01234567";
        let magnet = format!("magnet:?xt=urn:btih:{hex}");
        // Added from a .torrent file that has been removed since.
        let gone = dir
            .path()
            .join("gone.torrent")
            .to_string_lossy()
            .into_owned();
        let record = |key: String, uri: &str| {
            storage::SavedTorrent::new(
                key,
                "name".into(),
                uri.into(),
                "paused".into(),
                0,
                0,
                dir.path().into(),
            )
        };
        let mut saved = [
            record(engine::hash_uri(&magnet), &magnet),
            record("f".repeat(40), &gone),
        ];
        for torrent in &saved {
            storage.save_torrent(torrent).unwrap();
        }

        rekey_legacy_records(&storage, &mut saved);
        assert_eq!(saved[0].info_hash, hex);
        assert!(storage.load_torrent(hex).unwrap().is_some());
        assert_eq!(saved[1].info_hash, "f".repeat(40));
        assert!(storage.load_torrent(&"f".repeat(40)).unwrap().is_some());
    }

    #[test]
    fn storage_of_one_torrent_does_not_wait_for_another() {
        let runtime = storage_runtime().unwrap();
        // Stands in for a storage server busy with a slow disk.
        runtime.spawn(async { std::thread::sleep(Duration::from_secs(2)) });
        let (tx, rx) = std::sync::mpsc::channel();
        runtime.spawn(async move { tx.send(()).unwrap() });
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_ok());
        runtime.shutdown_background();
    }

    #[test]
    fn torrent_data_is_written_and_read_on_the_storage_runtime() {
        use mtorrent::utils::re_exports::mtorrent_base::data::new_async_storage;

        let dir = std::env::temp_dir().join(format!("rill-storage-{}", std::process::id()));
        let runtime = storage_runtime().unwrap();
        let (client, server) =
            new_async_storage(&dir, std::iter::once((4, PathBuf::from("file")))).unwrap();
        runtime.spawn(server.run());
        let data = runtime.block_on(async {
            client.write_block(0, vec![1, 2, 3, 4]).await.unwrap();
            client.read_block(0, 4).await.unwrap()
        });
        runtime.shutdown_background();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(data, [1, 2, 3, 4]);
    }
}

# Contributing

Bugs and ideas go to the [issue tracker](https://github.com/sachesi/rill/issues); security
problems do not, see [SECURITY.md](SECURITY.md).

Before a change goes in:

- `just check` and `just test` pass. CI runs both on Fedora 44, with `cargo deny check`, for
  every push and pull request.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
  `fix:`, `feat:`, `perf:`, `docs:` and so on, with a subject that says what changed for
  someone using Rill.
- Every string the user sees goes through `gettext`. `just po` updates the catalogues in
  `po/`, and a change that adds strings brings their translations along where it can.
- Behaviour described in `docs/` changes with the code that implements it.

## Where things are

    build.rs                 runs blueprint-compiler, bundles the GResource, compiles the
                             catalogues for running from the source tree
    data/ui/*.blp            the window, a torrent row, the add, details and preferences
                             dialogs, the shortcuts dialog
    data/style.css           the few rules libadwaita has no class for
    src/main.rs              locale, the single-instance check, then the database, the
                             runtimes mtorrent needs and the DHT node
    src/application.rs       AdwApplication subclass: app actions, opening files and links,
                             the tray's commands, pausing everything on shutdown
    src/window.rs            the list, search, selection mode, and the win.* actions the
                             rows and dialogs go through
    src/torrents.rs          what the window knows of each torrent, and the download queue
    src/torrent_row.rs       one row: its state, its button and its context menu
    src/dialogs/             add, details (with the Files page's list item) and preferences
    src/engine.rs            starts, pauses and stops mtorrent tasks on a thread of its own
    src/listener.rs          turns mtorrent's snapshots into updates for the window, and
                             reads the piece map from mtorrent's state file
    src/storage/             SQLite: torrents and settings, and the worker thread writes
                             go through
    src/torrent_paths.rs     where a torrent's content is, kept inside the download folder
    src/tray.rs              the StatusNotifier icon
    src/test_support.rs      what the tests share: scratch directories, a made-up .torrent,
                             an engine with its runtimes and DHT node

Widgets are GObject subclasses with composite templates from the Blueprint files. The
rows do not talk to the engine: their actions activate `win.pause-torrent`,
`win.resume-torrent` and `win.delete-torrent` with the torrent's info hash, and the window
does the rest.

mtorrent runs on Tokio, so the process lives inside a Tokio runtime and has two more: a
current-thread one for peers and a multi-threaded one for disk storage, plus the engine's
own thread. Updates reach the GTK main context through an `async-channel`. Database writes
on the paths the interface takes often are queued to the storage worker; the queue decides
from what the window keeps in memory and reads nothing.

## Running

    just run [FILE|MAGNET]   # debug build, translated from po/
    just check               # fmt, clippy -D warnings, blueprint, validators, catalogues
    just test                # the unit tests
    just msrv                # the build with the oldest Rust Rill supports, from rustup
    cargo deny check         # advisories, licences and sources of the dependencies

How much is logged is set in Preferences; `RUST_LOG` narrows it further, per module if
need be. `GTK_DEBUG=builder` reports template problems.

Rill keeps one instance per session bus. To try a change while an installed Rill is
running, quit that one first, or run the build on a bus of its own:

    dbus-run-session -- target/debug/rill

## A few things to know before changing them

User-visible strings go through `gettext` with `%s`-style placeholders and
`str::replace`; there is no printf from Rust. Counts use `ngettext` even where English
would not need it, because the plural rules of other languages do. `just pot` regenerates
the template from the Rust sources, the Blueprint files, the desktop entry and the
metainfo, and `just po` merges it into every `po/<lang>.po`. A new language is a new line
in `po/LINGUAS` plus the `.po` file.

A torrent's identity is its info hash, from the magnet link or the .torrent file; records
saved before that are re-keyed at startup. Names from a torrent never become paths
without `torrent_paths::contained_path`, and mtorrent refuses file paths that leave the
download folder.

# Installing

## What it needs

To build: Rust 1.95 or newer with Cargo, a C compiler (SQLite is compiled in),
`blueprint-compiler`, `just`, gettext, and the development packages of GTK 4.20 and
libadwaita 1.8. On Fedora:

    sudo dnf install rust cargo gcc just blueprint-compiler gettext gtk4-devel libadwaita-devel

To run: GTK 4.20 and libadwaita 1.8. The tray icon needs a StatusNotifier host, which
GNOME provides through the AppIndicator extension and most other desktops have built in.

## Building

    just build

builds `target/release/rill`. Translations are looked up under the prefix it is built for,
`/usr/local` unless `prefix` says otherwise, so give `build` the same prefix as `install`.

## Installing

    sudo just install

installs to `/usr/local`: the binary, the desktop entry, the metainfo, the icons and the
translations, and refreshes the desktop and icon caches. Another prefix:

    just prefix=$HOME/.local build install
    just prefix=/usr build && sudo just prefix=/usr install

`DESTDIR` stages an install for packaging:

    just prefix=/usr build
    DESTDIR=$PWD/stage just prefix=/usr install

The desktop entry registers Rill for `application/x-bittorrent` files and `magnet:` links.
To make it the default for both:

    xdg-mime default io.github.sachesi.rill.desktop application/x-bittorrent x-scheme-handler/magnet

Packages for Fedora, openSUSE, Debian, Ubuntu and Arch Linux, and how to install them, are in
the [README](../README.md#packages).

## Removing

    sudo just uninstall        # or with the prefix it was installed with

This leaves your torrents and settings in `~/.local/share/rill` and your downloads where
they are.

## Upgrading from 0.1

Rill 0.2 changed its application id from `com.github.sachesi.rill` to
`io.github.sachesi.rill`. Its data directory did not change, so torrents and settings carry
over, but files installed by the old `make install` stay behind. Remove them with:

    sudo rm -f /usr/share/applications/com.github.sachesi.rill.desktop \
        /usr/share/icons/hicolor/scalable/apps/com.github.sachesi.rill.svg \
        /usr/share/icons/hicolor/symbolic/apps/com.github.sachesi.rill-symbolic.svg

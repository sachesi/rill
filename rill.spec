%define _debugsource_template %{nil}
%define debug_package %{nil}

%global app_id io.github.sachesi.rill

Name:           rill
# The release workflow sets Version to the tag it builds; OBS counts the Release.
Version:        0.3.2
Release:        0
Summary:        Small BitTorrent client

License:        GPL-3.0-or-later AND Apache-2.0
URL:            https://github.com/sachesi/rill
# Named as the Debian source package names them, which OBS builds from the same files.
Source0:        %{url}/archive/refs/tags/v%{version}.tar.gz#/%{name}_%{version}.orig.tar.gz
# The crates the build needs, from the release, so that it runs without a network.
Source1:        %{url}/releases/download/v%{version}/%{name}-%{version}-vendor.tar.xz#/%{name}_%{version}.orig-vendor.tar.xz

BuildRequires:  cargo
BuildRequires:  rust >= 1.92
BuildRequires:  gcc
BuildRequires:  blueprint-compiler
BuildRequires:  desktop-file-utils
BuildRequires:  gettext-tools
BuildRequires:  AppStream
BuildRequires:  pkgconfig(gtk4) >= 4.20
BuildRequires:  pkgconfig(libadwaita-1) >= 1.8
BuildRequires:  pkgconfig(glib-2.0)
BuildRequires:  pkgconfig(xkbcommon)

Requires:       libgtk-4-1 >= 4.20
Requires:       libadwaita-1-0 >= 1.8
Requires:       hicolor-icon-theme

%description
Rill is a small BitTorrent client, built with GTK 4 and libadwaita on top
of mtorrent. It adds magnet links and .torrent files, groups transfers into
downloading, paused and finished with a limit on how many download at once,
shows each torrent's pieces, files, peers and trackers, and can download a
torrent sequentially. On desktops with a system tray the transfers go on
after the window is closed.

%prep
%autosetup -n %{name}-%{version} -b 1

%build
export CARGO_HOME="$PWD/.cargo-home"
export RUSTFLAGS="%{?build_rustflags}"
export RILL_LOCALEDIR="%{_datadir}/locale"
%if 0%{?_cargo_target_dir:1}
export CARGO_TARGET_DIR="%{_cargo_target_dir}"
%endif
cargo build --release --offline --locked

%install
%if 0%{?_cargo_target_dir:1}
target="%{_cargo_target_dir}/release"
%else
target="target/release"
%endif
install -Dpm 0755 "$target/rill" %{buildroot}%{_bindir}/rill

install -d %{buildroot}%{_datadir}/applications %{buildroot}%{_datadir}/metainfo
msgfmt --desktop --template=data/%{app_id}.desktop -d po \
  -o %{buildroot}%{_datadir}/applications/%{app_id}.desktop
msgfmt --xml --template=data/%{app_id}.metainfo.xml -d po \
  -o %{buildroot}%{_datadir}/metainfo/%{app_id}.metainfo.xml
install -Dpm 0644 data/icons/hicolor/scalable/apps/%{app_id}.svg \
  %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
install -Dpm 0644 data/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg \
  %{buildroot}%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg

for lang in $(cat po/LINGUAS); do
  install -d %{buildroot}%{_datadir}/locale/$lang/LC_MESSAGES
  msgfmt -o %{buildroot}%{_datadir}/locale/$lang/LC_MESSAGES/%{name}.mo po/$lang.po
done
%find_lang %{name}

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{app_id}.desktop
appstreamcli validate --no-net %{buildroot}%{_datadir}/metainfo/%{app_id}.metainfo.xml
test -x %{buildroot}%{_bindir}/rill

%files -f %{name}.lang
%license LICENSE
%doc README.md docs
%{_bindir}/rill
%{_datadir}/applications/%{app_id}.desktop
%{_datadir}/metainfo/%{app_id}.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg

%changelog

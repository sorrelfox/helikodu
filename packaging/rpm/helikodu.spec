# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Fedora RPM of Helikodu, built from the release source tarball, offline:
# the tarball carries every crate of Cargo.lock in vendor/ and a
# .cargo/config.toml that builds from them. The libraries come from Fedora.
# See docs/dev/packaging-rpm.md.
#
# The Release workflow passes the version of the tag:
#   rpmbuild -ba --define "upstream_version 0.9.0" helikodu.spec
# A suffix such as 0.9.0-rc1 becomes the RPM version 0.9.0~rc1, which sorts
# before 0.9.0.

%{!?upstream_version:%global upstream_version 0.9.0}

%global app_id io.github.sorrelfox.Helikodu

Name:           helikodu
Version:        %{lua: print((string.gsub(rpm.expand("%{upstream_version}"), "-", "~")))}
Release:        1%{?dist}
Summary:        Offline music player for large local libraries

# Helikodu itself is GPL-3.0-or-later. The programs link the Rust crates of
# vendor/ statically (cargo-vendor.txt lists them); the licenses of those
# built for Linux, from
#   cargo tree -p helikodu -p helikodu-cli -p helikodu-scan-worker \
#       --edges no-build,no-dev,no-proc-macro --target <arch>-unknown-linux-gnu \
#       --prefix none --format '{l}' | sort -u
License:        GPL-3.0-or-later AND Apache-2.0 AND (Apache-2.0 OR MIT) AND (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT) AND (0BSD OR Apache-2.0) AND (0BSD OR Apache-2.0 OR MIT) AND (Apache-2.0 OR BSD-3-Clause) AND BSD-3-Clause AND CC0-1.0 AND ISC AND MIT AND (Apache-2.0 OR MIT OR Zlib) AND Unicode-3.0 AND (MIT OR Unlicense) AND WTFPL AND Zlib
URL:            https://github.com/sorrelfox/helikodu
Source0:        %{url}/releases/download/v%{upstream_version}/%{name}-%{upstream_version}.tar.xz

# The workspace's minimum supported Rust version (rust-version in Cargo.toml).
BuildRequires:  rust >= 1.98
BuildRequires:  cargo
BuildRequires:  cargo-rpm-macros
BuildRequires:  meson >= 1.0
BuildRequires:  gcc
# The SID decoder's shim is C++.
BuildRequires:  gcc-c++
# bindgen, for the FFmpeg bindings.
BuildRequires:  clang-devel
BuildRequires:  pkgconf-pkg-config
BuildRequires:  gettext
BuildRequires:  python3
BuildRequires:  desktop-file-utils
BuildRequires:  libappstream-glib
BuildRequires:  appstream
BuildRequires:  pkgconfig(gtk4) >= 4.18
# Any FFmpeg 7.1 or newer: Fedora's ffmpeg-free-devel, or RPM Fusion's
# ffmpeg-devel. The package then needs the same libraries at run time, which
# libavcodec-free and friends (Fedora) and ffmpeg-libs (RPM Fusion) both
# provide.
BuildRequires:  pkgconfig(libavformat) >= 61
BuildRequires:  pkgconfig(libavcodec)
BuildRequires:  pkgconfig(libavutil)
BuildRequires:  pkgconfig(libpipewire-0.3) >= 1.2
BuildRequires:  pkgconfig(alsa)
# Only jack.pc, to build the JACK output; libjack is loaded at run time.
BuildRequires:  pkgconfig(jack)
# SQLite with FTS5, which Fedora's build has.
BuildRequires:  pkgconfig(sqlite3) >= 3.43
BuildRequires:  pkgconfig(libopenmpt)
BuildRequires:  pkgconfig(libgme)
BuildRequires:  pkgconfig(libsidplayfp)
# vgmstream is not packaged in Fedora, so its decoder is left out.

Requires:       hicolor-icon-theme
# Owns the D-Bus session service folder.
Requires:       dbus-common
# A General MIDI SoundFont, found at /usr/share/soundfonts/default.sf2, for
# MIDI files.
Recommends:     fluid-soundfont-gm
# The JACK output: PipeWire's libjack, loaded only when JACK is chosen.
Suggests:       pipewire-jack-audio-connection-kit

%description
Helikodu is a music player for Linux that works entirely offline. It keeps
a library of the music on your disks, with a fast search and smart
playlists, plays almost any audio format through FFmpeg, tracker modules,
game console music and C64 SID tunes, with gapless playback, ReplayGain,
crossfades and an equalizer, and has tools that edit tags, convert,
organize and sync files. It never connects to the network.

The package includes helikodu-cli, the command-line tool, and the user
guide. With Fedora's ffmpeg-free libraries a few formats do not play, among
them E-AC-3, WMA Pro and WMA Lossless; RPM Fusion's ffmpeg-libs adds them.

%prep
%autosetup -n %{name}-%{upstream_version}

%build
# Fedora's flags for Rust (optimization, debug information for the -debuginfo
# package, hardening); the profile's LTO setting stays. A build can add flags
# of its own (the Release workflow lowers the debug information to line
# tables, which keeps the link within its memory limit): rpmbuild --define
# "helikodu_extra_rustflags -C debuginfo=line-tables-only".
export RUSTFLAGS="%{build_rustflags}%{?helikodu_extra_rustflags: %{helikodu_extra_rustflags}}"
# The vendored crates, from the tarball's .cargo/config.toml.
%cargo_vendor_manifest
%meson -Doffline=true -Dvgmstream=false
%meson_build

%install
%meson_install
%find_lang %{name}

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{app_id}.desktop
appstream-util validate-relax --nonet %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml
%meson_test
%{buildroot}%{_bindir}/helikodu --version
%{buildroot}%{_bindir}/helikodu-cli --version

%files -f %{name}.lang
%license LICENSE cargo-vendor.txt
%doc README.md CHANGELOG.md
%{_bindir}/helikodu
%{_bindir}/helikodu-cli
%dir %{_prefix}/libexec/helikodu
%{_prefix}/libexec/helikodu/helikodu-scan-worker
%{_datadir}/applications/%{app_id}.desktop
%{_metainfodir}/%{app_id}.metainfo.xml
%{_datadir}/dbus-1/services/%{app_id}.service
%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg
%{_datadir}/helikodu/
%{_mandir}/man1/helikodu.1*
%{_mandir}/man1/helikodu-cli.1*
%{_mandir}/man1/helikodu-scan-worker.1*

%changelog
* %{lua: print(os.date("!%a %b %d %Y", tonumber(os.getenv("SOURCE_DATE_EPOCH")) or os.time()))} Helikodu contributors <https://github.com/sorrelfox/helikodu> - %{version}-%{release}
- Helikodu %{upstream_version}; the changes are in CHANGELOG.md

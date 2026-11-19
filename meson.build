# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Installs Helikodu: Cargo builds the programs, Meson installs them with the
# data files and the translations.
#
#   meson setup _build --prefix=/usr --buildtype=release
#   meson compile -C _build
#   meson install -C _build [--destdir DIR]
#
# The version is set in one place, [workspace.package] in Cargo.toml; it is
# read from there and only shown in the summary.

project(
  'helikodu',
  license: 'GPL-3.0-or-later',
  meson_version: '>= 1.0',
)

fs = import('fs')
i18n = import('i18n')
gnome = import('gnome')

app_id = 'io.github.sorrelfox.Helikodu'

cargo_toml = fs.read('Cargo.toml')
version = cargo_toml.split('[workspace.package]')[1].split('version = "')[1].split('"')[0]

prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
datadir = prefix / get_option('datadir')
localedir = prefix / get_option('localedir')
# The user guide is application data, not optional documentation: Help opens
# it, so it is installed where packages do not strip documentation.
guidedir = datadir / 'helikodu' / 'guide'

# The programs look for the scan worker beside themselves, in
# ../libexec/helikodu/ relative to their own folder, and in the folder set
# here at build time (find_worker_program in crates/library). It goes to
# <libexecdir>/helikodu unless the helperdir option names another folder,
# such as /usr/lib/helikodu on Arch Linux.
if get_option('helperdir') != ''
  worker_dir = prefix / get_option('helperdir')
else
  worker_dir = prefix / get_option('libexecdir') / 'helikodu'
endif

# --- The programs -----------------------------------------------------------

cargo = find_program('cargo')
sh = find_program('sh')

programs = ['helikodu', 'helikodu-cli', 'helikodu-scan-worker']

cargo_args = ['build', '--release', '--locked']
foreach program : programs
  cargo_args += ['--package', program]
endforeach
if get_option('offline')
  cargo_args += '--offline'
endif
features = []
if get_option('bundled_sqlite')
  # Feature unification passes it on to the library crate of every program.
  features += 'helikodu-cli/bundled-sqlite'
endif
if get_option('vgmstream')
  # Every program that decodes or scans gets the decoder; a build that cannot
  # find libvgmstream fails instead of leaving it out.
  features += ['helikodu/vgmstream', 'helikodu-cli/vgmstream', 'helikodu-scan-worker/vgmstream']
endif
if features.length() > 0
  cargo_args += ['--features', ','.join(features)]
endif

cargo_env = environment()
cargo_env.set('CARGO_TARGET_DIR', meson.current_build_dir() / 'cargo-target')
if get_option('vgmstream')
  cargo_env.set('HELIKODU_REQUIRE_VGMSTREAM', '1')
endif
# Where the GTK front end looks for the compiled translations.
cargo_env.set('HELIKODU_LOCALEDIR', localedir)
# Where the GTK front end finds the user guide for the Help menu.
cargo_env.set('HELIKODU_GUIDEDIR', guidedir)
# Where the programs look for the scan worker after the places beside them.
cargo_env.set('HELIKODU_HELPERDIR', worker_dir)

# Cargo runs in the source folder so that it reads .cargo/config.toml there
# (the vendored sources of a release tarball), then the programs are copied
# to where Meson expects its outputs.
cargo_script = '''
src="$1"; out="$(cd "$2" && pwd)"; shift 2
cd "$src"
"$CARGO" "$@"
for program in helikodu helikodu-cli helikodu-scan-worker; do
  cp "$CARGO_TARGET_DIR/release/$program" "$out/$program"
done
'''
cargo_env.set('CARGO', cargo.full_path())

custom_target(
  'cargo-build',
  output: programs,
  command: [sh, '-ec', cargo_script, 'sh', meson.project_source_root(), '@OUTDIR@', cargo_args],
  env: cargo_env,
  console: true,
  build_by_default: true,
  build_always_stale: true,
  install: true,
  install_dir: [bindir, bindir, worker_dir],
  install_mode: 'rwxr-xr-x',
)

# --- Data files -------------------------------------------------------------

# The desktop entry and the metainfo get the translations of po/ (both are
# listed in po/POTFILES.in).
desktop_file = i18n.merge_file(
  input: 'data' / app_id + '.desktop',
  output: app_id + '.desktop',
  type: 'desktop',
  po_dir: 'po',
  install: true,
  install_dir: datadir / 'applications',
)

metainfo_file = i18n.merge_file(
  input: 'data' / app_id + '.metainfo.xml',
  output: app_id + '.metainfo.xml',
  type: 'xml',
  po_dir: 'po',
  install: true,
  install_dir: datadir / 'metainfo',
)

service_conf = configuration_data()
service_conf.set('bindir', bindir)
configure_file(
  input: 'data' / app_id + '.service.in',
  output: app_id + '.service',
  configuration: service_conf,
  install_dir: datadir / 'dbus-1' / 'services',
)

install_data(
  'data/icons/hicolor/scalable/apps' / app_id + '.svg',
  install_dir: datadir / 'icons' / 'hicolor' / 'scalable' / 'apps',
)
install_data(
  'data/icons/hicolor/symbolic/apps' / app_id + '-symbolic.svg',
  install_dir: datadir / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)

install_man('data/man/helikodu.1', 'data/man/helikodu-cli.1', 'data/man/helikodu-scan-worker.1')

# The user guide as one HTML page, opened by Help › User Guide (F1) without a
# network. The script also fails on links to chapters or headings that do not
# exist.
python = import('python').find_installation('python3')
custom_target(
  'user-guide',
  input: files(
    'docs/user/build-guide.py',
    'docs/user/faq.md',
    'docs/user/guide/README.md',
    'docs/user/guide/audio-output.md',
    'docs/user/guide/browsing.md',
    'docs/user/guide/command-line.md',
    'docs/user/guide/converting-and-sync.md',
    'docs/user/guide/dsp.md',
    'docs/user/guide/expressions.md',
    'docs/user/guide/file-tools.md',
    'docs/user/guide/files.md',
    'docs/user/guide/first-run.md',
    'docs/user/guide/installing.md',
    'docs/user/guide/keyboard-shortcuts.md',
    'docs/user/guide/loudness.md',
    'docs/user/guide/moving-from-another-player.md',
    'docs/user/guide/playing.md',
    'docs/user/guide/playlists-and-ratings.md',
    'docs/user/guide/settings.md',
    'docs/user/guide/tags.md',
    'docs/user/guide/troubleshooting.md',
  ),
  output: 'index.html',
  command: [python, '@INPUT0@', '--out', '@OUTPUT@', '--version', version],
  build_by_default: true,
  install: true,
  install_dir: guidedir,
)

desktop_file_validate = find_program('desktop-file-validate', required: false)
if desktop_file_validate.found()
  test('desktop file', desktop_file_validate, args: [desktop_file])
endif
appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
  test(
    'metainfo',
    appstreamcli,
    args: ['validate', '--no-net', metainfo_file],
  )
endif

# --- Translations -----------------------------------------------------------

subdir('po')

# Refreshes the icon cache and the desktop database on a direct install
# (skipped when DESTDIR is set, as for packages) where the tools are installed.
gnome.post_install(
  gtk_update_icon_cache: find_program('gtk4-update-icon-cache', 'gtk-update-icon-cache', required: false).found(),
  update_desktop_database: find_program('update-desktop-database', required: false).found(),
)

summary(
  {
    'Version': version,
    'Prefix': prefix,
    'Scan worker': worker_dir,
    'Locale folder': localedir,
    'User guide': guidedir / 'index.html',
  },
  section: 'Install',
)
summary(
  {
    'Offline Cargo build': get_option('offline'),
    'Bundled SQLite': get_option('bundled_sqlite'),
    'vgmstream decoder': get_option('vgmstream'),
  },
  section: 'Build',
  bool_yn: true,
)

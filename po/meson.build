# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later

# Compiles the languages in LINGUAS to .mo files under localedir and adds the
# helikodu-pot and helikodu-update-po targets. An empty LINGUAS installs nothing.
i18n.gettext(
  meson.project_name(),
  preset: 'glib',
  args: ['--from-code=UTF-8', '--keyword=tr', '--keyword=ntr:1,2'],
)

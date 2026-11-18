#!/bin/sh
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the RPM from a release source tarball, inside a Fedora container
# (run as root there; it installs the build dependencies with dnf):
#
#   podman run --rm -v "$PWD:/src:Z" registry.fedoraproject.org/fedora:45 \
#       sh /src/packaging/rpm/build.sh /src/target/dist/helikodu-0.9.0.tar.xz /src/target/rpm
#
# Writes the binary RPM (and the -debuginfo, -debugsource and source RPMs)
# into the output folder and checks them with rpmlint.
#
# Environment: CARGO_BUILD_JOBS, SOURCE_DATE_EPOCH (default: the tarball's
# date), HELIKODU_EXTRA_RUSTFLAGS (Rust flags added after Fedora's).
set -eu

tarball=$(realpath "$1")
out=$2
spec_dir=$(dirname "$(realpath "$0")")

name=$(basename "$tarball" .tar.xz)
version=${name#helikodu-}

dnf install -y --setopt=install_weak_deps=False rpm-build rpmlint 'dnf-command(builddep)'

top=$(mktemp -d)
mkdir -p "$top/SOURCES" "$top/SPECS"
cp "$tarball" "$top/SOURCES/"
cp "$spec_dir/helikodu.spec" "$top/SPECS/"
spec="$top/SPECS/helikodu.spec"

dnf builddep -y --define "upstream_version $version" "$spec"

# The build date of the changelog entry and of the files.
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(stat -c %Y "$tarball")}
export SOURCE_DATE_EPOCH

# Rust flags added after Fedora's own, if any (the Release workflow passes
# -C debuginfo=line-tables-only to lower the build's memory peak).
if [ -n "${HELIKODU_EXTRA_RUSTFLAGS:-}" ]; then
    set -- --define "helikodu_extra_rustflags $HELIKODU_EXTRA_RUSTFLAGS"
else
    set --
fi

rpmbuild -ba \
    --define "_topdir $top" \
    --define "upstream_version $version" \
    --define "use_source_date_epoch_as_buildtime 1" \
    --define "clamp_mtime_to_source_date_epoch 1" \
    "$@" "$spec"

mkdir -p "$out"
find "$top/RPMS" "$top/SRPMS" -name '*.rpm' -exec cp {} "$out/" \;

rpmlint --file "$spec_dir/helikodu.rpmlintrc" "$spec" "$out"/*.rpm
ls -l "$out"

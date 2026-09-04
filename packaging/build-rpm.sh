#!/usr/bin/bash

set -euxo pipefail

VERSION=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')

RPM_ROOT="$(git rev-parse --show-toplevel)/packaging/rpmbuild"
mkdir -p "$RPM_ROOT"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}

SOURCE="$RPM_ROOT/SOURCES/halo-$VERSION.tar"
git archive --format=tar --prefix="halo-$VERSION/" "v$VERSION" > "$SOURCE"

cp packaging/halo.spec "$RPM_ROOT/SPECS/halo.spec"

rpmbuild -ba \
	--define "_topdir $RPM_ROOT" \
	--define "version $VERSION" \
	"$RPM_ROOT/SPECS/halo.spec"

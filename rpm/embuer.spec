Name:           embuer
Version:        %{version}
Release:        1%{?dist}
Summary:        Embuer - a small service and library for systemd / dbus

License:        GPLv2
URL:            https://github.com/neroreflex/embuer
BuildRequires:  cargo, gcc, clang, openssl-devel, pkgconfig, make
# Provide explicit runtime Requires to avoid empty macro expansion in some rpmbuild setups
Requires:       systemd, dbus

%description
Embuer provides a small service and library for managing systemd/dbus
integration for embedded workflows.

%prep
# No source tarball; build directly from the checked-out tree.
# rpmbuild will be invoked with `_sourcedir` pointing at the repo.
# Copy sources into the build directory so `%build` runs in the source tree
# (rpmbuild doesn't automatically chdir into the repository when _sourcedir
# is used this way).
rm -rf *
cp -a %{_sourcedir}/. .
# Ensure a clean build environment
rm -rf .git target || true

%build
export CARGO_HOME="$HOME/.cargo" || true
export PATH="$HOME/.cargo/bin:$PATH"
# Build all binaries in release mode (works with cross-target dirs)
cargo build --release --bins

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}/usr/bin
# Locate built binaries under target/**/release to support target-specific dirs
srv_bin=$(find target -type f -path "*/release/embuer-service" -print -quit)
clt_bin=$(find target -type f -path "*/release/embuer-client" -print -quit)
inst_bin=$(find target -type f -path "*/release/embuer-installer" -print -quit)
if [ -z "$srv_bin" ] || [ -z "$clt_bin" ] || [ -z "$inst_bin" ]; then
	echo "One or more built binaries not found under target/**/release" >&2
	exit 1
fi
install -m 755 "$srv_bin" %{buildroot}/usr/bin/embuer-service
install -m 755 "$clt_bin" %{buildroot}/usr/bin/embuer-client
install -m 755 "$inst_bin" %{buildroot}/usr/bin/embuer-installer
mkdir -p %{buildroot}/usr/lib
libso=$(find target -type f -path "*/release/libembuer.so" -print -quit)
liba=$(find target -type f -path "*/release/libembuer.a" -print -quit)
if [ -z "$libso" ] || [ -z "$liba" ]; then
	echo "Library artifacts not found under target/**/release" >&2
	exit 1
fi
install -m 644 "$libso" %{buildroot}/usr/lib/libembuer.so
install -m 644 "$liba" %{buildroot}/usr/lib/libembuer.a
mkdir -p %{buildroot}/usr/lib/systemd/system
install -m 644 rootfs/usr/lib/systemd/system/embuer.service %{buildroot}/usr/lib/systemd/system/embuer.service
mkdir -p %{buildroot}/usr/share/dbus-1/system.d
install -m 644 rootfs/usr/share/dbus-1/system.d/org.neroreflex.embuer.conf %{buildroot}/usr/share/dbus-1/system.d/org.neroreflex.embuer.conf

%files
%license LICENSE.md
%doc README.md
/usr/bin/embuer-service
/usr/bin/embuer-client
/usr/bin/embuer-installer
/usr/lib/libembuer.so
/usr/lib/libembuer.a
/usr/lib/systemd/system/embuer.service
/usr/share/dbus-1/system.d/org.neroreflex.embuer.conf

%changelog
* Thu Mar 05 2026 CI Build <ci@example.com> - %{version}-1
- Automated build: added changelog entry for reproducible build systems

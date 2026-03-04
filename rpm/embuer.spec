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

%build
rm Cargo.lock
export CARGO_HOME="$HOME/.cargo" || true
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}/usr/bin
install -m 755 target/release/embuer-service %{buildroot}/usr/bin/embuer-service
install -m 755 target/release/embuer-client %{buildroot}/usr/bin/embuer-client
install -m 755 target/release/embuer-installer %{buildroot}/usr/bin/embuer-installer
mkdir -p %{buildroot}/usr/lib
install -m 644 target/release/libembuer.so %{buildroot}/usr/lib/libembuer.so
install -m 644 target/release/libembuer.a %{buildroot}/usr/lib/libembuer.a
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

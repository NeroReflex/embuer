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
export CARGO_HOME="$HOME/.cargo" || true
export PATH="$HOME/.cargo/bin:$PATH"
# Build all binaries in release mode and place artifacts under the checked-out
# repository `target/` directory so `%install` can find `target/release/...`.
cargo build --release --manifest-path "%{_sourcedir}/Cargo.toml" --target-dir "%{_sourcedir}/target"

%install
ls -lah .
rm -rf %{buildroot}
mkdir -p %{buildroot}/usr/bin
# Install directly from the repository target dir where Cargo writes artifacts
srv_bin="%{_sourcedir}/target/release/embuer-service"
clt_bin="%{_sourcedir}/target/release/embuer-client"
inst_bin="%{_sourcedir}/target/release/embuer-installer"
if [ ! -f "$srv_bin" ] || [ ! -f "$clt_bin" ] || [ ! -f "$inst_bin" ]; then
	echo "Build artifacts missing in %{_sourcedir}/target/release; ensure cargo built with --target-dir %{_sourcedir}/target" >&2
	exit 1
fi
install -m 755 "$srv_bin" %{buildroot}/usr/bin/embuer-service
install -m 755 "$clt_bin" %{buildroot}/usr/bin/embuer-client
install -m 755 "$inst_bin" %{buildroot}/usr/bin/embuer-installer
mkdir -p %{buildroot}/usr/lib
libso="%{_sourcedir}/target/release/libembuer.so"
liba="%{_sourcedir}/target/release/libembuer.a"
if [ ! -f "$libso" ] || [ ! -f "$liba" ]; then
	echo "Library artifacts missing in %{_sourcedir}/target/release; ensure cargo built with --target-dir %{_sourcedir}/target" >&2
	exit 1
fi
install -m 644 "$libso" %{buildroot}/usr/lib/libembuer.so
install -m 644 "$liba" %{buildroot}/usr/lib/libembuer.a
mkdir -p %{buildroot}/usr/lib/systemd/system
install -m 644 %{_sourcedir}/rootfs/usr/lib/systemd/system/embuer.service %{buildroot}/usr/lib/systemd/system/embuer.service
mkdir -p %{buildroot}/usr/share/dbus-1/system.d
install -m 644 %{_sourcedir}/rootfs/usr/share/dbus-1/system.d/org.neroreflex.embuer.conf %{buildroot}/usr/share/dbus-1/system.d/org.neroreflex.embuer.conf

# Install documentation and license from the repository so %doc/%license work
mkdir -p %{buildroot}/usr/share/doc/embuer
if [ -f %{_sourcedir}/README.md ]; then
	install -m 644 %{_sourcedir}/README.md %{buildroot}/usr/share/doc/embuer/README.md
else
	echo "README.md missing in %{_sourcedir}; cannot populate %doc" >&2
	exit 1
fi

mkdir -p %{buildroot}/usr/share/licenses/embuer
if [ -f %{_sourcedir}/LICENSE.md ]; then
	install -m 644 %{_sourcedir}/LICENSE.md %{buildroot}/usr/share/licenses/embuer/LICENSE.md
else
	echo "LICENSE.md missing in %{_sourcedir}; cannot populate %license" >&2
	exit 1
fi

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

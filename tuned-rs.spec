Name:           tuned-rs
Epoch:          1
Version:        0.1.0
Release:        3%{?dist}
Summary:        Rust drop-in replacement for the TuneD system tuning daemon

Provides:       tuned = %{epoch}:%{version}-%{release}
Provides:       tuned%{?_isa} = %{epoch}:%{version}-%{release}
Obsoletes:      tuned < %{epoch}:%{version}-%{release}
Provides:       tuned-ppd = %{epoch}:%{version}-%{release}
Provides:       tuned-ppd%{?_isa} = %{epoch}:%{version}-%{release}
Obsoletes:      tuned-ppd < %{epoch}:%{version}-%{release}

License:        GPL-2.0-or-later
URL:            https://github.com/SisyphusAeolides/tuned-rs
Source0:        %{name}-%{version}.tar.gz
Source1:        vendor.tar.xz

BuildRequires:  cargo >= 1.75
BuildRequires:  make
BuildRequires:  rust >= 1.75
BuildRequires:  systemd-rpm-macros
BuildRequires:  systemd-devel
Requires:       dbus
Requires:       polkit
Requires:       systemd
Conflicts:      power-profiles-daemon

%description
tuned-rs implements the TuneD daemon and control interfaces in Rust. The
package owns the com.redhat.tuned D-Bus name, installs the classic tuned and
tuned-adm command names, loads existing profiles, and provides rollback and
power-profiles-daemon compatibility services.

%prep
%autosetup -p1 -a 1
mkdir -p .cargo
mv cargo-config.toml .cargo/config.toml

%build
CARGO_NET_OFFLINE=true CARGO_PROFILE_RELEASE_DEBUG=2 cargo build --frozen --release

%install
make install-bin DESTDIR=%{buildroot}
make install-data DESTDIR=%{buildroot} DOCDIR=%{_docdir}/%{name}

%post
%systemd_post tuned.service tuned-rs.service tuned-rs-ppd.service

%preun
%systemd_preun tuned.service tuned-rs.service tuned-rs-ppd.service

%postun
%systemd_postun_with_restart tuned.service tuned-rs.service tuned-rs-ppd.service

%check
CARGO_NET_OFFLINE=true cargo test --frozen --all-targets
make packaging-check

%files
%license LICENSE
%doc SUPPORT.md
%{_docdir}/%{name}/README.md
%{_bindir}/tuned-rs
%{_bindir}/tuned-rs-ppd
%{_sbindir}/tuned
%{_sbindir}/tuned-adm
%{_mandir}/man8/tuned-rs.8*
%{_mandir}/man8/tuned-adm.8*
%{_mandir}/man8/tuned-rs-ppd.8*
%{_unitdir}/tuned.service
%{_unitdir}/tuned-rs.service
%{_unitdir}/tuned-rs-ppd.service
%{_datadir}/dbus-1/system.d/com.redhat.tuned.conf
%{_datadir}/dbus-1/system-services/com.redhat.tuned.service
%{_datadir}/dbus-1/system.d/org.freedesktop.UPower.PowerProfiles.conf
%{_datadir}/dbus-1/system-services/org.freedesktop.UPower.PowerProfiles.service
%{_datadir}/dbus-1/system-services/net.hadess.PowerProfiles.service
%{_datadir}/polkit-1/actions/com.redhat.tuned.policy
%{_datadir}/polkit-1/actions/org.freedesktop.UPower.PowerProfiles.policy
%{_datadir}/polkit-1/actions/net.hadess.PowerProfiles.policy
%dir %{_sysconfdir}/tuned
%dir %{_sysconfdir}/tuned/profiles
%config(noreplace) %{_sysconfdir}/tuned/tuned-main.conf
%config(noreplace) %{_sysconfdir}/tuned/ppd.conf
%{_prefix}/lib/tuned/

%changelog
* Mon Jul 27 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 1:0.1.0-3
- Add the TuneD-compatible tuned-adm administration client
- Install classic tuned command, service, and D-Bus activation identities
- Replace tuned and tuned-ppd through versioned RPM capabilities

* Sun Jul 26 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 0.1.0-2
- Preserve debug information across Fedora and Enterprise Linux build roots

* Sun Jul 26 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 0.1.0-1
- Initial package

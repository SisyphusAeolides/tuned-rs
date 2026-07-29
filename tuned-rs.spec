Name:           tuned-rs
Epoch:          1
Version:        0.2.6
Release:        1%{?dist}
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
Requires:       xdg-utils
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
make install-bin DESTDIR=%{buildroot} BINDIR=%{_bindir} SBINDIR=%{_sbindir}
make install-data DESTDIR=%{buildroot} DOCDIR=%{_docdir}/%{name}

%post
%systemd_post tuned.service tuned-ppd.service

%preun
%systemd_preun tuned.service tuned-ppd.service

%postun
%systemd_postun_with_restart tuned.service tuned-ppd.service
if [ "$1" -eq 0 ]; then
  rm -f %{_sysconfdir}/grub.d/00_tuned || :
fi

%posttrans
if [ -d %{_sysconfdir}/grub.d ]; then
  cp -a %{_datadir}/tuned/grub2/00_tuned %{_sysconfdir}/grub.d/00_tuned
  restorecon %{_sysconfdir}/grub.d/00_tuned >/dev/null 2>&1 || :
fi

%check
CARGO_NET_OFFLINE=true cargo test --frozen --all-targets
make packaging-check

%files
%license LICENSE
%doc SUPPORT.md
%{_docdir}/%{name}/README.md
%{_bindir}/tuned-rs
%{_bindir}/tuned-rs-ppd
%{_bindir}/tuned-rs-gui
%{_sbindir}/tuned
%{_sbindir}/tuned-adm
%{_sbindir}/tuned-ppd
%{_mandir}/man8/tuned-rs.8*
%{_mandir}/man8/tuned-adm.8*
%{_mandir}/man8/tuned-rs-ppd.8*
%{_unitdir}/tuned.service
%{_unitdir}/tuned-rs.service
%{_unitdir}/tuned-ppd.service
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
%config(noreplace) %verify(not size mtime md5) %{_sysconfdir}/tuned/bootcmdline
%config(noreplace) %{_sysconfdir}/tuned/realtime-variables.conf
%config(noreplace) %{_sysconfdir}/tuned/realtime-virtual-guest-variables.conf
%config(noreplace) %{_sysconfdir}/tuned/realtime-virtual-host-variables.conf
%config(noreplace) %{_sysconfdir}/tuned/cpu-partitioning-variables.conf
%config(noreplace) %{_sysconfdir}/tuned/cpu-partitioning-powersave-variables.conf
%{_prefix}/lib/tuned/
%{_prefix}/lib/kernel/install.d/92-tuned.install
%{_datadir}/tuned/grub2/00_tuned
%{_datadir}/applications/tuned-rs-gui.desktop
%{_datadir}/icons/hicolor/scalable/apps/tuned-rs.svg
%{_datadir}/metainfo/io.github.SisyphusAeolides.tuned-rs.metainfo.xml

%changelog
* Wed Jul 29 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 1:0.2.6-1
- Reconcile firmware platform-profile drift with the selected TuneD profile
- Treat unavailable vendor-specific video controls as not applicable

* Tue Jul 28 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 1:0.2.5-1
- Honor verify --ignore-missing for vendor-specific video controls

* Tue Jul 28 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 1:0.2.4-1
- Tolerate rollback of devices removed since the profile was applied
- Avoid duplicate systemd alias restart jobs during package upgrades

* Tue Jul 28 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 1:0.2.3-1
- Honor merged-usr binary directories on Fedora

* Tue Jul 28 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 1:0.2.2-1
- Complete Rust 1.75 lint compatibility for the control center

* Tue Jul 28 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 1:0.2.1-1
- Preserve compatibility with the Rust 1.75 minimum toolchain

* Tue Jul 28 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 1:0.2.0-1
- Complete TuneD profile, plugin, control API, and configuration compatibility
- Add dynamic device tuning and the TuneD Control Center
- Add formal verification artifacts and expanded integration coverage

* Mon Jul 27 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 1:0.1.0-3
- Add the TuneD-compatible tuned-adm administration client
- Install classic tuned, tuned-ppd, service, and D-Bus activation identities
- Replace tuned and tuned-ppd through versioned RPM capabilities

* Sun Jul 26 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 0.1.0-2
- Preserve debug information across Fedora and Enterprise Linux build roots

* Sun Jul 26 2026 Kenny Glowner <SisyphusAeolides@pm.me> - 0.1.0-1
- Initial package

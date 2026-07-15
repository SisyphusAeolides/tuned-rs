Name:           tuned-rs
Version:        0.1.0
Release:        11%{?dist}
Summary:        Rust drop-in replacement for TuneD and Power Profiles Daemon

# Plain cargo release builds do not produce useful RPM debuginfo subpackages.
%global _enable_debug_packages 0
%global debug_package %{nil}
%global _debugsource_packages 0

License:        GPL-3.0-or-later
URL:            https://github.com/SisyphusAeolides/tuned-rs
Source0:        %{url}/archive/main.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  make
BuildRequires:  pkgconf-pkg-config
BuildRequires:  systemd-devel
BuildRequires:  systemd-rpm-macros

Provides:       tuned = %{version}
Provides:       power-profiles-daemon = %{version}
# Required to satisfy cosmic-settings ppd-service dependency
Provides:       ppd-service
# Required to allow dnf swap to automatically replace existing providers
Obsoletes:      power-profiles-daemon < %{version}
Obsoletes:      tuned-ppd < %{version}

Conflicts:      tuned
Conflicts:      tuned-ppd
Conflicts:      power-profiles-daemon

Recommends:     forged

%description
tuned-rs is a memory-safe Rust implementation of the TuneD daemon and the
Power Profiles D-Bus API (PPD). It exposes com.redhat.tuned for tuned-adm and
net.hadess.PowerProfiles / org.freedesktop.UPower.PowerProfiles for desktop
power mode controls. Hermes plugin should enable only when /dev/hermes exists.
When forged is installed, %post may drop a forge unit into /etc/forge/units/.

%prep
%autosetup -n tuned-rs-main

%build
cargo build --release

%install
%make_install BINDIR=%{_sbindir}
install -d %{buildroot}%{_datadir}/tuned-rs/forge
install -m 0644 forge/tuned-rs.forge.toml \
  %{buildroot}%{_datadir}/tuned-rs/forge/tuned-rs.forge.toml

%post
%systemd_post tuned-rs.service tuned-rs-ppd.service
if [ -d /etc/forge/units ] && [ -f %{_datadir}/tuned-rs/forge/tuned-rs.forge.toml ]; then
  cp -n %{_datadir}/tuned-rs/forge/tuned-rs.forge.toml \
    /etc/forge/units/61-tuned-rs.forge.toml 2>/dev/null || true
fi

%preun
%systemd_preun tuned-rs.service tuned-rs-ppd.service

%postun
%systemd_postun_with_restart tuned-rs.service tuned-rs-ppd.service

%files
%dir %{_sysconfdir}/tuned/profiles
%config(noreplace) %{_sysconfdir}/tuned/tuned-main.conf
%config(noreplace) %{_sysconfdir}/tuned/ppd.conf
%{_prefix}/lib/tuned/profiles
%license %{_docdir}/tuned-rs/README.md
%{_docdir}/tuned-rs/selinux/tuned-rs.fc
%{_docdir}/tuned-rs/selinux/tuned-rs.te
%{_sbindir}/tuned-rs
%{_sbindir}/tuned-rs-ppd
%{_unitdir}/tuned-rs.service
%{_unitdir}/tuned-rs-ppd.service
%{_datadir}/dbus-1/system.d/com.redhat.tuned.conf
%{_datadir}/dbus-1/system.d/org.freedesktop.UPower.PowerProfiles.conf
%{_datadir}/dbus-1/system-services/net.hadess.PowerProfiles.service
%{_datadir}/dbus-1/system-services/org.freedesktop.UPower.PowerProfiles.service
%{_datadir}/polkit-1/actions/com.redhat.tuned.policy
%{_datadir}/polkit-1/actions/net.hadess.PowerProfiles.policy
%{_datadir}/polkit-1/actions/org.freedesktop.UPower.PowerProfiles.policy
%dir %{_datadir}/tuned-rs
%dir %{_datadir}/tuned-rs/forge
%{_datadir}/tuned-rs/forge/tuned-rs.forge.toml

%changelog
* Wed Jul 15 2026 Kenny Glowner <sisyphuscode@fedoraproject.org> - 0.1.0-11
- Ship forge unit and %post install into /etc/forge/units/ when forged is present

* Sun Jun 28 2026 Kenneth Glowner <klglownerjr@usmarinecorps.vet> - 0.1.0-10
- Added Provides: ppd-service and Obsoletes for PPD/tuned-ppd to fix dependency resolution.

* Wed Jun 24 2026 Kenneth Glowner <klglownerjr@usmarinecorps.vet> - 0.1.0-9
- Stop firmware platform_profile changes from overriding a user-selected PPD
  profile and rewriting ppd_base_profile.
- Default ppd.conf to sysfs_acpi_monitor=false for stable manual profile choice.

* Wed Jun 24 2026 Kenneth Glowner <klglownerjr@usmarinecorps.vet> - 0.1.0-8
- Pick CPU governors and EPP values from hardware-available options.
- Avoid duplicate cpufreq writes and skip unchanged CPU sysfs values.

* Wed Jun 24 2026 Kenneth Glowner <klglownerjr@usmarinecorps.vet> - 0.1.0-7
- Ship ppd.conf, tuned-main.conf, and default TuneD profiles required for a
  standalone install without the Python tuned package.
- Fix profile search paths to match upstream TuneD layout.

* Wed Jun 24 2026 Kenneth Glowner <klglownerjr@usmarinecorps.vet> - 0.1.0-6
- Add RHEL 10 / EPEL 10 / CentOS Stream 10 COPR chroot support.

* Tue Jun 23 2026 Kenneth Glowner <klglownerjr@usmarinecorps.vet> - 0.1.0-5
- Install binaries via %{_sbindir} so Fedora usr-merge and EL9 both match %files.

Name:           tuned-rs
Version:        0.2.10
Release:        %autorelease
Summary:        Rust drop-in replacement for the TuneD system tuning daemon

Provides:       tuned = %{version}-%{release}
Provides:       tuned%{?_isa} = %{version}-%{release}
Obsoletes:      tuned < %{version}-%{release}
Provides:       tuned-ppd = %{version}-%{release}
Provides:       tuned-ppd%{?_isa} = %{version}-%{release}
Obsoletes:      tuned-ppd < %{version}-%{release}

# Main package license: GPL-2.0-or-later AND MIT AND Apache-2.0
# Vendor dependency licenses:
# Apache-2.0 OR MIT
# Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT
# BSD-2-Clause OR Apache-2.0 OR MIT
# GPL-2.0-or-later
# MIT
# MIT OR Apache-2.0
# MIT OR Apache-2.0 OR LGPL-2.1-or-later
# MIT OR LGPL-3.0-or-later
# Unlicense OR MIT
License:        %{shrink:
                GPL-2.0-or-later AND MIT AND Apache-2.0 AND
                (Apache-2.0 OR MIT) AND
                (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT) AND
                (BSD-2-Clause OR Apache-2.0 OR MIT) AND
                (MIT OR Apache-2.0 OR LGPL-2.1-or-later) AND
                (MIT OR LGPL-3.0-or-later) AND
                (Unlicense OR MIT)
                }
URL:            https://github.com/SisyphusAeolides/tuned-rs
Source0:        %{name}-%{version}.tar.gz
# Vendored dependencies are used because packaging all dependencies would require an unreasonable amount of work.
Source1:        vendor.tar.xz

%if 0%{?fedora}
BuildRequires:  cargo-rpm-macros >= 24
%endif
%if 0%{?rhel} >= 9
BuildRequires:  rust-toolset >= 1.75
%else
BuildRequires:  cargo >= 1.75
BuildRequires:  rust >= 1.75
%endif
BuildRequires:  gcc
BuildRequires:  make
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
%if 0%{?fedora}
# Tells Fedora's build system to use your existing vendor.tar.xz directory
%cargo_prep -v vendor
%else
mkdir -p .cargo
mv cargo-config.toml .cargo/config.toml
%endif

%build
%if 0%{?fedora}
# Replaces manual cargo commands and injects Fedora's hardening flags (PIE, RELRO)
%cargo_build
# Prints the required multi-license string to the build log
%{cargo_license_summary}
%else
CARGO_NET_OFFLINE=true CARGO_PROFILE_RELEASE_DEBUG=2 cargo build --frozen --release
%endif

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
%if 0%{?fedora}
# Replaces manual cargo commands with the offline-compatible macro
%cargo_test
%else
CARGO_NET_OFFLINE=true cargo test --frozen --all-targets
%endif
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
%autochangelog
%bcond_without check

Name:           tuned-rs
Version:        0.1.0
Release:        1%{?dist}
Summary:        High-performance Rust rewrite of the TuneD system tuning daemon

License:        GPL-2.0-or-later
URL:            https://github.com/SisyphusAeolides/tuned-rs
Source:         %{url}/archive/v%{version}/%{name}-%{version}.tar.gz

BuildRequires:  cargo-rpm-macros >= 24
BuildRequires:  make
BuildRequires:  systemd-rpm-macros

%description
High-performance Rust rewrite of the TuneD system tuning daemon with advanced
features beyond the original. Drop-in D-Bus API compatibility with
com.redhat.tuned and com.redhat.tuned.control.

%prep
%autosetup -p1
%if ! 0%{?copr_username:1}
%cargo_prep
%endif

%if ! 0%{?copr_username:1}
%generate_buildrequires
%cargo_generate_buildrequires
%endif

%build
%if 0%{?copr_username:1}
cargo build --release
%else
%cargo_build
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies
%endif

%install
%if 0%{?copr_username:1}
install -D -m 0755 target/release/tuned-rs %{buildroot}%{_bindir}/tuned-rs
install -D -m 0755 target/release/tuned-rs-ppd %{buildroot}%{_bindir}/tuned-rs-ppd
%else
%cargo_install
%endif
make install-data DESTDIR=%{buildroot} DOCDIR=%{_docdir}/%{name}

%if %{with check}
%check
%if ! 0%{?copr_username:1}
%cargo_test
%endif
%endif

%files
%if ! 0%{?copr_username:1}
%license LICENSE.dependencies
%endif
%{_docdir}/%{name}/README.md
%{_bindir}/tuned-rs
%{_bindir}/tuned-rs-ppd
%{_unitdir}/tuned-rs.service
%{_unitdir}/tuned-rs-ppd.service
%{_datadir}/dbus-1/system.d/com.redhat.tuned.conf
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
* Sat Jul 25 2026 Kenny Glowner <SisyphusAeolides@users.noreply.github.com> - 0.1.0-1
- Initial package

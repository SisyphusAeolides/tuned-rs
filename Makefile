PREFIX ?= /usr
BINDIR ?= $(PREFIX)/bin
SBINDIR ?= $(PREFIX)/sbin
SYSTEMDUNITDIR ?= /usr/lib/systemd/system
DBUSCONFDIR ?= /usr/share/dbus-1/system.d
DBUSSERVICEDIR ?= /usr/share/dbus-1/system-services
POLKITDIR ?= /usr/share/polkit-1/actions
DOCDIR ?= /usr/share/doc/tuned-rs
MANDIR ?= /usr/share/man
ETCTUNEDDIR ?= /etc/tuned
PROFILEDIR ?= /usr/lib/tuned/profiles
TUNEDDATADIR ?= /usr/share/tuned
KERNELINSTALLDIR ?= /usr/lib/kernel/install.d
APPLICATIONDIR ?= /usr/share/applications
ICONDIR ?= /usr/share/icons/hicolor/scalable/apps
METAINFO_DIR ?= /usr/share/metainfo

.PHONY: all build test check packaging-check proofs proofs-strict install install-bin install-data install-config install-profiles tarball vendor srpm

all: build

tarball:
	git archive --format=tar.gz --prefix=tuned-rs-0.2.7/ --output=tuned-rs-0.2.7.tar.gz HEAD

vendor:
	builddir="$$(mktemp -d)"; \
	cd "$$builddir"; \
	cargo vendor --locked --manifest-path="$(CURDIR)/Cargo.toml" vendor > cargo-config.toml; \
	tar -cJf "$(CURDIR)/vendor.tar.xz" vendor cargo-config.toml; \
	rm -rf "$$builddir"

srpm: tarball vendor
	rpmbuild -bs --define "_sourcedir $(PWD)" --define "_srcrpmdir $(PWD)" tuned-rs.spec

build:
	cargo build --locked --release

check: packaging-check
	cargo fmt --all -- --check
	cargo check --locked --all-targets
	cargo clippy --locked --all-targets -- -D warnings

packaging-check:
	grep -q '^name = "tuned-adm"' Cargo.toml
	grep -q '^name = "tuned-rs-gui"' Cargo.toml
	grep -Eq '^Provides: +tuned( |%)' tuned-rs.spec
	grep -Eq '^Obsoletes: +tuned( |<)' tuned-rs.spec
	grep -Eq '^Provides: +tuned-ppd( |%)' tuned-rs.spec
	grep -Eq '^Obsoletes: +tuned-ppd( |<)' tuned-rs.spec
	grep -q '%{_sbindir}/tuned-adm' tuned-rs.spec
	grep -q '%{_sbindir}/tuned-ppd' tuned-rs.spec
	grep -q '%{_unitdir}/tuned.service' tuned-rs.spec
	grep -q '%{_unitdir}/tuned-ppd.service' tuned-rs.spec
	grep -q 'ExecStart=/usr/sbin/tuned' packaging/tuned-rs.service
	grep -q 'ExecStart=/usr/sbin/tuned-ppd' packaging/tuned-rs-ppd.service
	! grep -q 'Conflicts=.*tuned.service' packaging/tuned-rs.service
	! grep -q 'Conflicts=.*tuned-ppd.service' packaging/tuned-rs-ppd.service
	test -f packaging/com.redhat.tuned.service
	test -f packaging/tuned-adm.8
	test -x packaging/00_tuned.grub
	test -x packaging/92-tuned.install
	test -f packaging/bootcmdline
	test -f packaging/tuned-rs-gui.desktop
	test -f packaging/io.github.SisyphusAeolides.tuned-rs.metainfo.xml
	test -f assets/icons/tuned-circle-gauge.svg
	test -f profiles/realtime/realtime-variables.conf
	test -f profiles/realtime-virtual-guest/realtime-virtual-guest-variables.conf
	test -f profiles/realtime-virtual-host/realtime-virtual-host-variables.conf
	test -f profiles/cpu-partitioning/cpu-partitioning-variables.conf
	test -f profiles/cpu-partitioning-powersave/cpu-partitioning-powersave-variables.conf
	grep -q 'realtime-variables.conf' tuned-rs.spec
	grep -q 'cpu-partitioning-variables.conf' tuned-rs.spec

test:
	cargo test --locked --all-targets
	$(MAKE) proofs

proofs:
	sh scripts/check-formal.sh

proofs-strict:
	sh scripts/check-formal.sh --strict

install: build install-bin install-data

install-bin:
	install -D -m 0755 target/release/tuned-rs $(DESTDIR)$(BINDIR)/tuned-rs
	install -d $(DESTDIR)$(SBINDIR)
	ln -sfn ../bin/tuned-rs $(DESTDIR)$(SBINDIR)/tuned
	install -D -m 0755 target/release/tuned-adm $(DESTDIR)$(SBINDIR)/tuned-adm
	install -D -m 0755 target/release/tuned-rs-ppd $(DESTDIR)$(BINDIR)/tuned-rs-ppd
	ln -sfn ../bin/tuned-rs-ppd $(DESTDIR)$(SBINDIR)/tuned-ppd
	install -D -m 0755 target/release/tuned-rs-gui $(DESTDIR)$(BINDIR)/tuned-rs-gui

install-data: install-config install-profiles
	install -D -m 0644 packaging/tuned-rs.service $(DESTDIR)$(SYSTEMDUNITDIR)/tuned-rs.service
	ln -sfn tuned-rs.service $(DESTDIR)$(SYSTEMDUNITDIR)/tuned.service
	install -D -m 0644 packaging/com.redhat.tuned.conf $(DESTDIR)$(DBUSCONFDIR)/com.redhat.tuned.conf
	install -D -m 0644 packaging/com.redhat.tuned.service $(DESTDIR)$(DBUSSERVICEDIR)/com.redhat.tuned.service
	install -D -m 0644 packaging/com.redhat.tuned.policy $(DESTDIR)$(POLKITDIR)/com.redhat.tuned.policy
	install -D -m 0644 README.md $(DESTDIR)$(DOCDIR)/README.md
	install -D -m 0644 packaging/tuned-rs-ppd.service $(DESTDIR)$(SYSTEMDUNITDIR)/tuned-rs-ppd.service
	ln -sfn tuned-rs-ppd.service $(DESTDIR)$(SYSTEMDUNITDIR)/tuned-ppd.service
	install -D -m 0644 packaging/org.freedesktop.UPower.PowerProfiles.conf $(DESTDIR)$(DBUSCONFDIR)/org.freedesktop.UPower.PowerProfiles.conf
	install -D -m 0644 packaging/org.freedesktop.UPower.PowerProfiles.service $(DESTDIR)$(DBUSSERVICEDIR)/org.freedesktop.UPower.PowerProfiles.service
	install -D -m 0644 packaging/net.hadess.PowerProfiles.service $(DESTDIR)$(DBUSSERVICEDIR)/net.hadess.PowerProfiles.service
	install -D -m 0644 packaging/org.freedesktop.UPower.PowerProfiles.policy $(DESTDIR)$(POLKITDIR)/org.freedesktop.UPower.PowerProfiles.policy
	install -D -m 0644 packaging/net.hadess.PowerProfiles.policy $(DESTDIR)$(POLKITDIR)/net.hadess.PowerProfiles.policy
	install -D -m 0644 packaging/tuned-rs.8 $(DESTDIR)$(MANDIR)/man8/tuned-rs.8
	install -D -m 0644 packaging/tuned-adm.8 $(DESTDIR)$(MANDIR)/man8/tuned-adm.8
	install -D -m 0644 packaging/tuned-rs-ppd.8 $(DESTDIR)$(MANDIR)/man8/tuned-rs-ppd.8
	install -D -m 0755 packaging/00_tuned.grub $(DESTDIR)$(TUNEDDATADIR)/grub2/00_tuned
	install -D -m 0755 packaging/92-tuned.install $(DESTDIR)$(KERNELINSTALLDIR)/92-tuned.install
	install -D -m 0644 packaging/tuned-rs-gui.desktop $(DESTDIR)$(APPLICATIONDIR)/tuned-rs-gui.desktop
	install -D -m 0644 assets/icons/tuned-circle-gauge.svg $(DESTDIR)$(ICONDIR)/tuned-rs.svg
	install -D -m 0644 packaging/io.github.SisyphusAeolides.tuned-rs.metainfo.xml $(DESTDIR)$(METAINFO_DIR)/io.github.SisyphusAeolides.tuned-rs.metainfo.xml

install-config:
	install -d $(DESTDIR)$(ETCTUNEDDIR)/profiles
	install -D -m 0644 packaging/tuned-main.conf $(DESTDIR)$(ETCTUNEDDIR)/tuned-main.conf
	install -D -m 0644 packaging/ppd.conf $(DESTDIR)$(ETCTUNEDDIR)/ppd.conf
	install -D -m 0644 packaging/bootcmdline $(DESTDIR)$(ETCTUNEDDIR)/bootcmdline
	install -D -m 0644 profiles/realtime/realtime-variables.conf $(DESTDIR)$(ETCTUNEDDIR)/realtime-variables.conf
	install -D -m 0644 profiles/realtime-virtual-guest/realtime-virtual-guest-variables.conf $(DESTDIR)$(ETCTUNEDDIR)/realtime-virtual-guest-variables.conf
	install -D -m 0644 profiles/realtime-virtual-host/realtime-virtual-host-variables.conf $(DESTDIR)$(ETCTUNEDDIR)/realtime-virtual-host-variables.conf
	install -D -m 0644 profiles/cpu-partitioning/cpu-partitioning-variables.conf $(DESTDIR)$(ETCTUNEDDIR)/cpu-partitioning-variables.conf
	install -D -m 0644 profiles/cpu-partitioning-powersave/cpu-partitioning-powersave-variables.conf $(DESTDIR)$(ETCTUNEDDIR)/cpu-partitioning-powersave-variables.conf
	find $(DESTDIR)$(ETCTUNEDDIR)/profiles -type d -exec chmod 0755 {} + 2>/dev/null || true

install-profiles:
	install -d $(DESTDIR)$(PROFILEDIR)
	cp -a profiles/. $(DESTDIR)$(PROFILEDIR)/
	find $(DESTDIR)$(PROFILEDIR) -type d -exec chmod 0755 {} +
	find $(DESTDIR)$(PROFILEDIR) -type f -exec chmod 0644 {} +
	find $(DESTDIR)$(PROFILEDIR) -type f -name '*.sh' -exec chmod 0755 {} +

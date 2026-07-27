PREFIX ?= /usr
BINDIR ?= $(PREFIX)/bin
SYSTEMDUNITDIR ?= /usr/lib/systemd/system
DBUSCONFDIR ?= /usr/share/dbus-1/system.d
DBUSSERVICEDIR ?= /usr/share/dbus-1/system-services
POLKITDIR ?= /usr/share/polkit-1/actions
DOCDIR ?= /usr/share/doc/tuned-rs
MANDIR ?= /usr/share/man
ETCTUNEDDIR ?= /etc/tuned
PROFILEDIR ?= /usr/lib/tuned/profiles

.PHONY: all build test check proofs proofs-strict install install-bin install-data install-config install-profiles tarball vendor srpm

all: build

tarball:
	git archive --format=tar.gz --prefix=tuned-rs-0.1.0/ --output=tuned-rs-0.1.0.tar.gz HEAD

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

check:
	cargo check --locked
	cargo clippy --locked -- -D warnings

test:
	cargo test --locked
	$(MAKE) proofs

proofs:
	sh scripts/check-formal.sh

proofs-strict:
	sh scripts/check-formal.sh --strict

install: build install-bin install-data

install-bin:
	install -D -m 0755 target/release/tuned-rs $(DESTDIR)$(BINDIR)/tuned-rs
	install -D -m 0755 target/release/tuned-rs-ppd $(DESTDIR)$(BINDIR)/tuned-rs-ppd

install-data: install-config install-profiles
	install -D -m 0644 packaging/tuned-rs.service $(DESTDIR)$(SYSTEMDUNITDIR)/tuned-rs.service
	install -D -m 0644 packaging/com.redhat.tuned.conf $(DESTDIR)$(DBUSCONFDIR)/com.redhat.tuned.conf
	install -D -m 0644 packaging/com.redhat.tuned.policy $(DESTDIR)$(POLKITDIR)/com.redhat.tuned.policy
	install -D -m 0644 README.md $(DESTDIR)$(DOCDIR)/README.md
	install -D -m 0644 packaging/tuned-rs-ppd.service $(DESTDIR)$(SYSTEMDUNITDIR)/tuned-rs-ppd.service
	install -D -m 0644 packaging/org.freedesktop.UPower.PowerProfiles.conf $(DESTDIR)$(DBUSCONFDIR)/org.freedesktop.UPower.PowerProfiles.conf
	install -D -m 0644 packaging/org.freedesktop.UPower.PowerProfiles.service $(DESTDIR)$(DBUSSERVICEDIR)/org.freedesktop.UPower.PowerProfiles.service
	install -D -m 0644 packaging/net.hadess.PowerProfiles.service $(DESTDIR)$(DBUSSERVICEDIR)/net.hadess.PowerProfiles.service
	install -D -m 0644 packaging/org.freedesktop.UPower.PowerProfiles.policy $(DESTDIR)$(POLKITDIR)/org.freedesktop.UPower.PowerProfiles.policy
	install -D -m 0644 packaging/net.hadess.PowerProfiles.policy $(DESTDIR)$(POLKITDIR)/net.hadess.PowerProfiles.policy
	install -D -m 0644 packaging/tuned-rs.8 $(DESTDIR)$(MANDIR)/man8/tuned-rs.8
	install -D -m 0644 packaging/tuned-rs-ppd.8 $(DESTDIR)$(MANDIR)/man8/tuned-rs-ppd.8

install-config:
	install -d $(DESTDIR)$(ETCTUNEDDIR)/profiles
	install -D -m 0644 packaging/tuned-main.conf $(DESTDIR)$(ETCTUNEDDIR)/tuned-main.conf
	install -D -m 0644 packaging/ppd.conf $(DESTDIR)$(ETCTUNEDDIR)/ppd.conf
	find $(DESTDIR)$(ETCTUNEDDIR)/profiles -type d -exec chmod 0755 {} + 2>/dev/null || true

install-profiles:
	install -d $(DESTDIR)$(PROFILEDIR)
	cp -a profiles/. $(DESTDIR)$(PROFILEDIR)/
	find $(DESTDIR)$(PROFILEDIR) -type d -exec chmod 0755 {} +
	find $(DESTDIR)$(PROFILEDIR) -type f -exec chmod 0644 {} +

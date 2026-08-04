.DEFAULT_GOAL := srpm

.PHONY: srpm

name := tuned-rs
version := $(shell awk 'tolower($$1) == "version:" { print $$2; exit }' $(spec))
builddir := $(shell mktemp -d)

srpm:
	dnf -y install cargo rust git rpm-build
	mkdir -p $(outdir)
	git archive --format=tar.gz --prefix=$(name)-$(version)/ --output=$(builddir)/$(name)-$(version).tar.gz HEAD
	cd $(builddir) && cargo vendor --locked --manifest-path=$(CURDIR)/Cargo.toml vendor > cargo-config.toml
	tar -C $(builddir) -cJf $(builddir)/vendor.tar.xz vendor cargo-config.toml
	rpmbuild -bs --define "_sourcedir $(builddir)" --define "_srcrpmdir $(outdir)" $(spec)

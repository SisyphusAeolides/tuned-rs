.DEFAULT_GOAL := srpm

.PHONY: srpm

name := tuned-rs
version := 0.1.0
builddir := $(shell mktemp -d)

srpm:
	dnf -y install cargo rust rpm-build
	mkdir -p $(outdir)
	git archive --format=tar.gz --prefix=$(name)-$(version)/ --output=$(builddir)/$(name)-$(version).tar.gz HEAD
	cd $(builddir) && cargo vendor --locked --manifest-path=$(CURDIR)/Cargo.toml vendor > cargo-config.toml
	tar -C $(builddir) -cJf $(builddir)/vendor.tar.xz vendor cargo-config.toml
	rpmbuild -bs --define "_sourcedir $(builddir)" --define "_srcrpmdir $(outdir)" $(spec)

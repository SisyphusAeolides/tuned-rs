.DEFAULT_GOAL := srpm

.PHONY: srpm

srpm:
	mkdir -p $(outdir)
	git archive --format=tar.gz --prefix=tuned-rs-0.1.0/ HEAD > tuned-rs-0.1.0.tar.gz
	cargo vendor --locked vendor
	tar -cJf vendor.tar.xz vendor/
	cp $(spec) .copr/tuned-rs-copr.spec
	sed -i 's/^Source:.*$$/&\nSource1: vendor.tar.xz/' .copr/tuned-rs-copr.spec
	sed -i 's/%autosetup -p1/%autosetup -p1 -a 1/' .copr/tuned-rs-copr.spec
	sed -i 's/%cargo_prep/%cargo_prep -V 1/' .copr/tuned-rs-copr.spec
	sed -i '/%generate_buildrequires/d' .copr/tuned-rs-copr.spec
	sed -i '/%cargo_generate_buildrequires/d' .copr/tuned-rs-copr.spec
	rpmbuild -bs --define "_sourcedir $$(pwd)" --define "_srcrpmdir $(outdir)" .copr/tuned-rs-copr.spec

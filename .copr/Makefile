.DEFAULT_GOAL := srpm

.PHONY: srpm

srpm:
	mkdir -p $(outdir)
	tar --exclude=.git --exclude=tuned-rs-0.1.0.tar.gz -czf tuned-rs-0.1.0.tar.gz --transform="s,^\.,tuned-rs-0.1.0," .
	curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
	export PATH="$$HOME/.cargo/bin:$$PATH" && cargo vendor --locked vendor
	tar -cJf vendor.tar.xz vendor/
	cp $(spec) .copr/tuned-rs-copr.spec
	sed -i 's/^Source:.*$$/&\nSource1: vendor.tar.xz/' .copr/tuned-rs-copr.spec
	sed -i 's/%autosetup -p1/%autosetup -p1 -a 1/' .copr/tuned-rs-copr.spec
	sed -i 's/%cargo_prep/%cargo_prep -V 1/' .copr/tuned-rs-copr.spec
	sed -i '/%generate_buildrequires/d' .copr/tuned-rs-copr.spec
	sed -i '/%cargo_generate_buildrequires/d' .copr/tuned-rs-copr.spec
	rpmbuild -bs --define "_sourcedir $$(pwd)" --define "_srcrpmdir $(outdir)" .copr/tuned-rs-copr.spec

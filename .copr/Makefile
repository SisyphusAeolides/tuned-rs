.PHONY: srpm

srpm:
	dnf install -y rpm-build git
	mkdir -p $(outdir)
	git archive --format=tar.gz --prefix=tuned-rs-0.1.0/ HEAD > tuned-rs-0.1.0.tar.gz
	rpmbuild -bs --define "_sourcedir $(PWD)" --define "_srcrpmdir $(outdir)" tuned-rs.spec

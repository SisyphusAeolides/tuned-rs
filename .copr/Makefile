.PHONY: srpm

srpm:
	mkdir -p $(outdir)
	git archive --format=tar.gz --prefix=tuned-rs-0.1.0/ HEAD > tuned-rs-0.1.0.tar.gz
	cargo vendor
	tar -cJf vendor.tar.xz vendor/
	sed -i 's/^Source:.*$/&\nSource1: vendor.tar.xz/' tuned-rs.spec
	sed -i 's/%autosetup -p1/%autosetup -p1 -a 1/' tuned-rs.spec
	sed -i 's/%cargo_prep/%cargo_prep -V 1/' tuned-rs.spec
	sed -i '/%generate_buildrequires/d' tuned-rs.spec
	sed -i '/%cargo_generate_buildrequires/d' tuned-rs.spec
	rpmbuild -bs --define "_sourcedir $$(pwd)" --define "_srcrpmdir $(outdir)" tuned-rs.spec

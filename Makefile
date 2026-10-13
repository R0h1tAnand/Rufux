PREFIX ?= /usr/local
CARGO ?= cargo

.PHONY: all build release test clean run install

all: release

build:
	$(CARGO) build

release:
	$(CARGO) build --release

test:
	$(CARGO) test

run:
	$(CARGO) run --release

clean:
	$(CARGO) clean

install: release
	install -d $(DESTDIR)$(PREFIX)/bin
	install -m 755 target/release/rufux $(DESTDIR)$(PREFIX)/bin/rufux
	install -d $(DESTDIR)/usr/share/applications
	install -m 644 rufux.desktop $(DESTDIR)/usr/share/applications/rufux.desktop
	install -d $(DESTDIR)/usr/share/polkit-1/actions
	install -m 644 org.rufux.policy $(DESTDIR)/usr/share/polkit-1/actions/org.rufux.policy
	install -d $(DESTDIR)/usr/share/icons/hicolor/128x128/apps
	install -m 644 assets/icons/rufus-128.png $(DESTDIR)/usr/share/icons/hicolor/128x128/apps/rufux.png || true

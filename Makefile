PREFIX ?= /usr/local

build:
	cargo build --release

install: build
	install -d $(DESTDIR)$(PREFIX)/bin
	install -m 755 target/release/rill $(DESTDIR)$(PREFIX)/bin/
	install -d $(DESTDIR)$(PREFIX)/share/applications
	install -m 644 packaging/usr/share/applications/com.github.sachesi.rill.desktop \
		$(DESTDIR)$(PREFIX)/share/applications/
	install -d $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps
	install -m 644 packaging/usr/share/icons/hicolor/scalable/apps/com.github.sachesi.rill.svg \
		$(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/
	gtk-update-icon-cache -f -t $(DESTDIR)$(PREFIX)/share/icons/hicolor 2>/dev/null || true
	@echo "Installed. Run 'rill' to start."
	@echo "Uninstall: make uninstall"

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/rill
	rm -f $(DESTDIR)$(PREFIX)/share/applications/com.github.sachesi.rill.desktop
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/com.github.sachesi.rill.svg
	gtk-update-icon-cache -f -t $(DESTDIR)$(PREFIX)/share/icons/hicolor 2>/dev/null || true
	@echo "Uninstalled."

.PHONY: build install uninstall

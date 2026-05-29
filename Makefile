# ── Rill — Minimalistic BitTorrent client for GNOME ──────────────────────────
PREFIX  ?= /usr
CARGO   := cargo

# ── Development ──────────────────────────────────────────────────────────────

.PHONY: help
help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
	  | sort \
	  | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-16s\033[0m %s\n", $$1, $$2}'

.PHONY: build
build: ## Debug build
	$(CARGO) build 

.PHONY: release
release: ## Optimised release build
	$(CARGO) build --release

.PHONY: run
run: ## Build and launch (debug)
	$(CARGO) run

.PHONY: check
check: ## Fast compile-check without codegen
	$(CARGO) check

.PHONY: test
test: ## Run tests
	$(CARGO) test

.PHONY: clean
clean: ## Remove build artifacts
	$(CARGO) clean

# ── Lint & format ────────────────────────────────────────────────────────────

.PHONY: clippy
clippy: ## Run clippy lints
	$(CARGO) clippy -- -D warnings

.PHONY: fmt
fmt: ## Format source (check-only)
	$(CARGO) fmt -- --check

.PHONY: fix
fix: ## Apply rustfmt
	$(CARGO) fmt

# ── Install ──────────────────────────────────────────────────────────────────

DESKTOP_SRC  := resources/rill.desktop
DESKTOP_NAME := com.github.sachesi.rill.desktop
ICON_SRC     := resources/icons/hicolor/scalable/apps/com.github.sachesi.rill.svg
ICON_NAME    := com.github.sachesi.rill.svg

target/release/rill:
	$(CARGO) build --release

.PHONY: install
install: target/release/rill ## Install under $(PREFIX) (set DESTDIR for staging)
	install -d $(DESTDIR)$(PREFIX)/bin
	install -m 755 target/release/rill $(DESTDIR)$(PREFIX)/bin/
	install -d $(DESTDIR)$(PREFIX)/share/applications
	install -m 644 $(DESKTOP_SRC) $(DESTDIR)$(PREFIX)/share/applications/$(DESKTOP_NAME)
	install -d $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps
	install -m 644 $(ICON_SRC) $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/$(ICON_NAME)
	-gtk-update-icon-cache -f -t $(DESTDIR)$(PREFIX)/share/icons/hicolor
	-update-desktop-database $(DESTDIR)$(PREFIX)/share/applications
	@echo "✓ Installed. Run 'rill' to start."
	@echo "  Uninstall: make uninstall"

.PHONY: uninstall
uninstall: ## Remove installed files
	rm -f $(DESTDIR)$(PREFIX)/bin/rill
	rm -f $(DESTDIR)$(PREFIX)/share/applications/$(DESKTOP_NAME)
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/$(ICON_NAME)
	-gtk-update-icon-cache -f -t $(DESTDIR)$(PREFIX)/share/icons/hicolor
	-update-desktop-database $(DESTDIR)$(PREFIX)/share/applications
	@echo "✓ Uninstalled."

SHELL := /usr/bin/bash
.DELETE_ON_ERROR:

MAKEFILE_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
PROJECT_DIR  ?= $(patsubst %/,%,$(MAKEFILE_DIR))
SPECFILE     ?= $(or $(spec),$(PROJECT_DIR)/rill.spec)
NAME         ?= rill

RPMBUILD_DIR ?= $(HOME)/rpmbuild
SOURCES_DIR  ?= $(RPMBUILD_DIR)/SOURCES
SRPMS_DIR    ?= $(RPMBUILD_DIR)/SRPMS
RPMS_DIR     ?= $(RPMBUILD_DIR)/RPMS
OUTDIR       ?= $(or $(outdir),$(SRPMS_DIR))

CARGO_TARGET_DIR ?= $(HOME)/.cache/rpmbuild-cargo-target/$(NAME)

VERSION := $(shell rpmspec -q --qf '%{VERSION}\n' --srpm "$(SPECFILE)" 2>/dev/null | head -n1)

SOURCE_ARCHIVE := $(SOURCES_DIR)/$(NAME)-$(VERSION).tar.gz
VENDOR_NAME    := $(NAME)-$(VERSION)-vendor.tar.zst
VENDOR_PATH    := $(SOURCES_DIR)/$(VENDOR_NAME)

SPECTOOL ?= rpmdev-spectool

.PHONY: all rpm srpm ba bs rpm-local srpm-local ba-local bs-local copr vendor sources local-sources prepare clean clean-cargo info check

all: srpm

rpm: ba
srpm: bs

rpm-local: ba-local
srpm-local: bs-local

ba: sources
	rpmbuild -ba --without vendored \
		--define "_topdir $(RPMBUILD_DIR)" \
		--define "_sourcedir $(SOURCES_DIR)" \
		--define "_cargo_target_dir $(CARGO_TARGET_DIR)" \
		"$(SPECFILE)"

bs: sources
	rpmbuild -bs --without vendored \
		--define "_topdir $(RPMBUILD_DIR)" \
		--define "_sourcedir $(SOURCES_DIR)" \
		--define "_srcrpmdir $(OUTDIR)" \
		"$(SPECFILE)"

ba-local: local-sources
	rpmbuild -ba --without vendored \
		--define "_topdir $(RPMBUILD_DIR)" \
		--define "_sourcedir $(SOURCES_DIR)" \
		--define "_cargo_target_dir $(CARGO_TARGET_DIR)" \
		"$(SPECFILE)"

bs-local: local-sources
	rpmbuild -bs --without vendored \
		--define "_topdir $(RPMBUILD_DIR)" \
		--define "_sourcedir $(SOURCES_DIR)" \
		--define "_srcrpmdir $(OUTDIR)" \
		"$(SPECFILE)"

sources: check prepare
	$(SPECTOOL) -g -C "$(SOURCES_DIR)" "$(SPECFILE)"

local-sources: check prepare
	@command -v rsync >/dev/null || { echo "ERROR: rsync not found." >&2; exit 1; }
	@echo "creating local Source0: $(SOURCE_ARCHIVE)"
	@tmpdir="$$(mktemp -d)"; \
	trap 'rm -rf "$$tmpdir"' EXIT; \
	mkdir -p "$$tmpdir/$(NAME)-$(VERSION)"; \
	rsync -rt --delete \
		--chmod=Du=rwx,Dgo=rx,Fu=rw,Fgo=r \
		--exclude='.git' --exclude='.gitignore' --exclude='.copr' \
		--exclude='.local' --exclude='result' --exclude='results' \
		--exclude='dist' --exclude='build' --exclude='target' \
		--exclude='vendor' --exclude='.cargo' --exclude='.cargo-home' \
		--exclude='__pycache__' --exclude='*.pyc' \
		"$(PROJECT_DIR)/" "$$tmpdir/$(NAME)-$(VERSION)/"; \
	tar --owner=0 --group=0 --numeric-owner \
		-C "$$tmpdir" -czf "$(SOURCE_ARCHIVE)" "$(NAME)-$(VERSION)"
	@echo "local Source0 ready: $(SOURCE_ARCHIVE)"

vendor: local-sources
	@echo "creating vendor tarball: $(VENDOR_PATH)"
	@tmpdir="$$(mktemp -d)"; \
	trap 'rm -rf "$$tmpdir"' EXIT; \
	root="$$(tar -tf "$(SOURCE_ARCHIVE)" | head -n1 | cut -d/ -f1)"; \
	test -n "$$root" || { echo "ERROR: could not detect archive root" >&2; exit 1; }; \
	tar -xf "$(SOURCE_ARCHIVE)" -C "$$tmpdir"; \
	cd "$$tmpdir/$$root"; \
	rm -f rust-toolchain.toml; \
	mkdir -p .cargo; \
	cargo vendor vendor > .cargo/config.toml; \
	test -s .cargo/config.toml || { echo "ERROR: cargo vendor did not create .cargo/config.toml" >&2; exit 1; }; \
	test -d vendor || { echo "ERROR: vendor directory missing" >&2; exit 1; }; \
	tar --owner=0 --group=0 --numeric-owner --zstd \
		-cf "$(VENDOR_PATH)" vendor .cargo/config.toml
	@echo "vendor archive ready: $(VENDOR_PATH)"

copr: vendor
	rpmbuild -bs --with vendored \
		--define "_topdir $(RPMBUILD_DIR)" \
		--define "_sourcedir $(SOURCES_DIR)" \
		--define "_srcrpmdir $(OUTDIR)" \
		"$(SPECFILE)"
	@srpm="$$(ls -1t "$(OUTDIR)"/$(NAME)-$(VERSION)-*.src.rpm | head -n1)"; \
	test -n "$$srpm" || { echo "ERROR: no SRPM found in $(OUTDIR)" >&2; exit 1; }; \
	echo "verifying $$srpm"; \
	rpm -qpl "$$srpm" | grep -F "$(VENDOR_NAME)" >/dev/null || { \
		echo "ERROR: SRPM does not include $(VENDOR_NAME)" >&2; exit 1; }; \
	echo "OK: SRPM includes $(VENDOR_NAME)"

prepare:
	@mkdir -p "$(SOURCES_DIR)" "$(SRPMS_DIR)" "$(RPMS_DIR)" "$(OUTDIR)"

check:
	@test -f "$(SPECFILE)" || { echo "ERROR: spec not found: $(SPECFILE)" >&2; exit 1; }
	@test -n "$(VERSION)" || { echo "ERROR: could not read Version from $(SPECFILE)" >&2; exit 1; }
	@command -v rpmspec >/dev/null || { echo "ERROR: rpmspec not found. Install rpm-build." >&2; exit 1; }
	@command -v rpmbuild >/dev/null || { echo "ERROR: rpmbuild not found. Install rpm-build." >&2; exit 1; }
	@command -v $(SPECTOOL) >/dev/null || { echo "ERROR: $(SPECTOOL) not found. Install rpmdevtools." >&2; exit 1; }
	@command -v cargo >/dev/null || { echo "ERROR: cargo not found. Install rust/cargo." >&2; exit 1; }
	@command -v tar >/dev/null || { echo "ERROR: tar not found." >&2; exit 1; }
	@command -v zstd >/dev/null || { echo "ERROR: zstd not found. Install zstd." >&2; exit 1; }

info:
	@echo "NAME:             $(NAME)"
	@echo "VERSION:          $(VERSION)"
	@echo "PROJECT_DIR:      $(PROJECT_DIR)"
	@echo "SPECFILE:         $(SPECFILE)"
	@echo "RPMBUILD_DIR:     $(RPMBUILD_DIR)"
	@echo "SOURCES_DIR:      $(SOURCES_DIR)"
	@echo "SRPMS_DIR:        $(SRPMS_DIR)"
	@echo "RPMS_DIR:         $(RPMS_DIR)"
	@echo "OUTDIR:           $(OUTDIR)"
	@echo "SPECTOOL:         $(SPECTOOL)"
	@echo "SOURCE_ARCHIVE:   $(SOURCE_ARCHIVE)"
	@echo "VENDOR_PATH:      $(VENDOR_PATH)"
	@echo "CARGO_TARGET_DIR: $(CARGO_TARGET_DIR)"

clean:
	rm -f "$(SOURCE_ARCHIVE)" "$(VENDOR_PATH)"

clean-cargo:
	rm -rf "$(CARGO_TARGET_DIR)"

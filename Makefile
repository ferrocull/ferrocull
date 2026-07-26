# GNU-standard install layout. A user install is the same target with a
# different prefix: `make install prefix=$HOME/.local`.
PREFIX ?= /usr/local
prefix ?= $(PREFIX)
exec_prefix ?= $(prefix)
bindir ?= $(exec_prefix)/bin
datarootdir ?= $(prefix)/share
datadir ?= $(datarootdir)

INSTALL ?= install
INSTALL_PROGRAM ?= $(INSTALL) -Dm755
INSTALL_DATA ?= $(INSTALL) -Dm644

APP_ID := io.github.ferrocull.Ferrocull
ICON_SIZES := 128x128 256x256

appsdir := $(DESTDIR)$(datadir)/applications
icondir := $(DESTDIR)$(datadir)/icons/hicolor

binary := target/release/ferrocull
sources := $(shell find crates -name '*.rs') Cargo.toml Cargo.lock

.PHONY: build install uninstall update-caches

build: $(binary)

# Refuse to compile under sudo: cargo would litter target/ with root-owned
# artifacts and rebuild against root's registry cache.
$(binary): $(sources)
	@if [ -n "$$SUDO_USER" ] || [ -n "$$DOAS_USER" ]; then \
		echo "$(binary) is missing or stale. Run 'make build' as your own user first." >&2; \
		exit 1; \
	fi
	cargo build --release

install: $(binary)
	$(INSTALL_PROGRAM) $(binary) $(DESTDIR)$(bindir)/ferrocull
	$(INSTALL_DATA) assets/$(APP_ID).desktop $(appsdir)/$(APP_ID).desktop
	$(INSTALL_DATA) logo.svg $(icondir)/scalable/apps/$(APP_ID).svg
	@for size in $(ICON_SIZES); do \
		$(INSTALL_DATA) "assets/icons/$$size.png" \
			"$(icondir)/$$size/apps/$(APP_ID).png" || exit 1; \
	done
	@$(MAKE) --no-print-directory update-caches

uninstall:
	rm -f $(DESTDIR)$(bindir)/ferrocull
	rm -f $(appsdir)/$(APP_ID).desktop
	rm -f $(icondir)/scalable/apps/$(APP_ID).svg
	@for size in $(ICON_SIZES); do \
		rm -f "$(icondir)/$$size/apps/$(APP_ID).png" || exit 1; \
	done
	@$(MAKE) --no-print-directory update-caches

# Distro packaging runs these itself once the staged tree is unpacked, so skip
# them whenever DESTDIR points at a staging root.
update-caches:
	@if [ -z "$(DESTDIR)" ] && command -v update-desktop-database >/dev/null 2>&1; then \
		update-desktop-database "$(appsdir)"; \
	fi
	@if [ -z "$(DESTDIR)" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then \
		gtk-update-icon-cache --ignore-theme-index "$(icondir)"; \
	fi

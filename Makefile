APP_ID    := com.ravenfilemanager.Raven
BIN_NAME  := ravenfilemanager
PREFIX    ?= /usr/local
BINDIR    := $(PREFIX)/bin
DATADIR   := $(PREFIX)/share
ICONDIR   := $(DATADIR)/icons/hicolor/scalable/apps
DESTDIR   ?=

PROFILE   ?= release
CARGO_FLAGS := $(if $(filter release,$(PROFILE)),--release,)

TARGET_DIR := target/$(PROFILE)
BINARY     := $(TARGET_DIR)/$(BIN_NAME)

.PHONY: all build run test clean install uninstall

all: build

build:
	cargo build $(CARGO_FLAGS)

run: build
	$(BINARY)

test:
	cargo test --workspace

clean:
	cargo clean

# Refreshing these is what makes the entry usable rather than merely present:
# without mimeinfo.cache nothing offers Raven for inode/directory, and a stale
# icon cache hides the icon from anything that trusts the cache over the
# directory. Skipped under DESTDIR, where the tree is staged rather than live
# and the packager runs these itself; best-effort otherwise, because neither
# tool is required for a working install.
define update-caches
	@if [ -z "$(DESTDIR)" ]; then \
		command -v update-desktop-database >/dev/null 2>&1 && \
			update-desktop-database -q "$(DATADIR)/applications" || true; \
		command -v gtk-update-icon-cache >/dev/null 2>&1 && \
			gtk-update-icon-cache -qtf "$(DATADIR)/icons/hicolor" || true; \
	fi
endef

install: build
	@echo "Installing $(BIN_NAME) to $(DESTDIR)$(BINDIR)..."
	install -Dm755 $(BINARY)                                    "$(DESTDIR)$(BINDIR)/$(BIN_NAME)"
	install -Dm644 data/$(APP_ID).desktop                       "$(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop"
	install -Dm644 data/$(APP_ID).metainfo.xml                  "$(DESTDIR)$(DATADIR)/metainfo/$(APP_ID).metainfo.xml"
	install -Dm644 data/icons/hicolor/scalable/apps/$(APP_ID).svg "$(DESTDIR)$(ICONDIR)/$(APP_ID).svg"
	install -Dm644 config/default.toml                          "$(DESTDIR)$(DATADIR)/$(BIN_NAME)/config/default.toml"
	install -Dm644 config/keybindings.toml                      "$(DESTDIR)$(DATADIR)/$(BIN_NAME)/config/keybindings.toml"
	install -Dm644 config/actions.toml                          "$(DESTDIR)$(DATADIR)/$(BIN_NAME)/config/actions.toml"
	install -Dm644 data/resources/style.css                     "$(DESTDIR)$(DATADIR)/$(BIN_NAME)/resources/style.css"
	install -Dm644 data/resources/resources.gresource.xml       "$(DESTDIR)$(DATADIR)/$(BIN_NAME)/resources/resources.gresource.xml"
	$(update-caches)
	@echo "Installation complete."

uninstall:
	@echo "Uninstalling $(BIN_NAME)..."
	rm -f  "$(DESTDIR)$(BINDIR)/$(BIN_NAME)"
	rm -f  "$(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop"
	rm -f  "$(DESTDIR)$(DATADIR)/metainfo/$(APP_ID).metainfo.xml"
	rm -f  "$(DESTDIR)$(ICONDIR)/$(APP_ID).svg"
	rm -rf "$(DESTDIR)$(DATADIR)/$(BIN_NAME)"
	$(update-caches)
	@echo "Uninstall complete."

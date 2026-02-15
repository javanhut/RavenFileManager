APP_ID    := com.ravenfilemanager.Raven
BIN_NAME  := raven
PREFIX    ?= /usr/local
BINDIR    := $(PREFIX)/bin
DATADIR   := $(PREFIX)/share
DESTDIR   ?=

PROFILE   ?= release
CARGO_FLAGS := $(if $(filter release,$(PROFILE)),--release,)

TARGET_DIR := target/$(PROFILE)
BINARY     := $(TARGET_DIR)/$(BIN_NAME)

.PHONY: all build clean install uninstall

all: build

build:
	cargo build $(CARGO_FLAGS)

clean:
	cargo clean
	rm -rf target

install: build
	install -Dm755 $(BINARY)                                    $(DESTDIR)$(BINDIR)/$(BIN_NAME)
	install -Dm644 data/$(APP_ID).desktop                       $(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop
	install -Dm644 data/$(APP_ID).metainfo.xml                  $(DESTDIR)$(DATADIR)/metainfo/$(APP_ID).metainfo.xml
	install -Dm644 config/default.toml                          $(DESTDIR)$(DATADIR)/$(BIN_NAME)/config/default.toml
	install -Dm644 config/keybindings.toml                      $(DESTDIR)$(DATADIR)/$(BIN_NAME)/config/keybindings.toml
	install -Dm644 config/actions.toml                          $(DESTDIR)$(DATADIR)/$(BIN_NAME)/config/actions.toml
	install -Dm644 data/resources/style.css                     $(DESTDIR)$(DATADIR)/$(BIN_NAME)/resources/style.css
	install -Dm644 data/resources/resources.gresource.xml       $(DESTDIR)$(DATADIR)/$(BIN_NAME)/resources/resources.gresource.xml

uninstall:
	rm -f  $(DESTDIR)$(BINDIR)/$(BIN_NAME)
	rm -f  $(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop
	rm -f  $(DESTDIR)$(DATADIR)/metainfo/$(APP_ID).metainfo.xml
	rm -rf $(DESTDIR)$(DATADIR)/$(BIN_NAME)

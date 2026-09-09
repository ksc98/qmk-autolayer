PREFIX ?= $(HOME)/.cargo
BIN     = $(PREFIX)/bin/qmk-autolayer
UNIT    = qmk-autolayer.service
UNITDIR = $(HOME)/.config/systemd/user

.PHONY: build install uninstall start stop restart status logs test lint

build:
	cargo build --release --locked

# Build, install the binary, install and enable the user service.
install:
	cargo install --path . --locked --force --root $(PREFIX)
	install -Dm644 $(UNIT) $(UNITDIR)/$(UNIT)
	systemctl --user daemon-reload
	systemctl --user enable --now $(UNIT)

# Stop and remove the service and the binary.
uninstall:
	-systemctl --user disable --now $(UNIT)
	rm -f $(UNITDIR)/$(UNIT)
	systemctl --user daemon-reload
	rm -f $(BIN)

start:
	systemctl --user start $(UNIT)

stop:
	systemctl --user stop $(UNIT)

restart:
	systemctl --user restart $(UNIT)

status:
	systemctl --user status $(UNIT) --no-pager

logs:
	journalctl --user -u qmk-autolayer -f

test:
	cargo test --locked

lint:
	cargo fmt --check
	cargo clippy --all-targets --locked -- -D warnings

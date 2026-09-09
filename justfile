set shell := ["bash", "-uc"]

default:
    @just --list

# Build + install the binary to ~/.cargo/bin, install the systemd user unit, (re)start it.
install:
    cargo install --path . --locked --force
    install -Dm644 qmk-autolayer.service ~/.config/systemd/user/qmk-autolayer.service
    systemctl --user daemon-reload
    systemctl --user enable --now qmk-autolayer.service
    systemctl --user restart qmk-autolayer.service

# Stop and remove the unit (the binary stays).
uninstall:
    systemctl --user disable --now qmk-autolayer.service || true
    rm -f ~/.config/systemd/user/qmk-autolayer.service
    systemctl --user daemon-reload

# Follow the daemon's log.
tail:
    journalctl --user -u qmk-autolayer -f

test:
    cargo test --locked

lint:
    cargo clippy --all-targets --locked -- -D warnings
    cargo fmt --check

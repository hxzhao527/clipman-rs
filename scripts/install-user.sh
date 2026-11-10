#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
data_home=${XDG_DATA_HOME:-"$HOME/.local/share"}
bin_home=${XDG_BIN_HOME:-"$HOME/.local/bin"}

cd "$repo_root"
cargo build --release

install -Dm755 target/release/clipman-rs "$bin_home/clipman-rs"
install -Dm644 packaging/io.github.clipman_rs.Clipman.desktop \
    "$data_home/applications/io.github.clipman_rs.Clipman.desktop"
install -Dm644 packaging/icons/hicolor/scalable/apps/io.github.clipman_rs.Clipman.svg \
    "$data_home/icons/hicolor/scalable/apps/io.github.clipman_rs.Clipman.svg"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data_home/applications"
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t "$data_home/icons/hicolor" || true
fi

printf 'Installed clipman-rs to %s\n' "$bin_home"

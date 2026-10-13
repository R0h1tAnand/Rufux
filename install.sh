#!/bin/sh
# Rufux installer.
#
#   curl -fsSL https://raw.githubusercontent.com/R0h1tAnand/Rufux/main/install.sh | sh
#
# Options (pass after `sh -s --` when piping):
#   --prefix DIR    install under DIR instead of /usr/local
#   --no-deps       do not install system packages
#   --uninstall     remove a previous installation
#
# Environment: RUFUX_REF selects the git branch or tag to build (default: main).
#
# The script builds as the current user and uses sudo only for the steps that need root
# (package installation and copying files into system directories).

set -eu

REPO_URL="https://github.com/R0h1tAnand/Rufux.git"
REF="${RUFUX_REF:-main}"
PREFIX="${PREFIX:-/usr/local}"
DATADIR="/usr/share"
MIN_RUST="1.92"
DESTDIR="${DESTDIR:-}"
INSTALL_DEPS=1
UNINSTALL=0

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# Run a command as root, via sudo when needed (never when staging into DESTDIR).
as_root() {
    if [ "$(id -u)" -eq 0 ] || [ -n "$DESTDIR" ]; then
        "$@"
    elif have sudo; then
        sudo "$@"
    else
        die "root privileges are required but sudo is not installed; re-run as root"
    fi
}

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --prefix) [ $# -ge 2 ] || die "--prefix needs a directory"; PREFIX="$2"; shift ;;
            --prefix=*) PREFIX="${1#--prefix=}" ;;
            --no-deps) INSTALL_DEPS=0 ;;
            --uninstall) UNINSTALL=1 ;;
            -h|--help) sed -n '2,15p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
            *) die "unknown option: $1" ;;
        esac
        shift
    done
    case "$PREFIX" in /*) ;; *) die "--prefix must be an absolute path" ;; esac
}

check_platform() {
    [ "$(uname -s)" = "Linux" ] || die "Rufux only supports Linux"
    case "$(uname -m)" in x86_64|aarch64|arm64) ;; *) warn "untested architecture: $(uname -m)" ;; esac
}

install_packages() {
    [ "$INSTALL_DEPS" -eq 1 ] || { say "Skipping system packages (--no-deps)"; return; }
    say "Installing system packages"
    if have apt-get; then
        as_root apt-get update
        # polkit's pkexec is packaged as "pkexec" on current releases, "policykit-1" on older ones.
        polkit_pkg="pkexec"
        apt-cache show pkexec >/dev/null 2>&1 || polkit_pkg="policykit-1"
        as_root apt-get install -y build-essential pkg-config curl git parted dosfstools ntfs-3g \
            exfatprogs e2fsprogs p7zip-full zenity "$polkit_pkg" udev util-linux \
            libxkbcommon-dev libwayland-dev libgl1-mesa-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev
    elif have dnf; then
        as_root dnf install -y gcc pkgconf curl git parted dosfstools ntfs-3g exfatprogs e2fsprogs \
            p7zip p7zip-plugins zenity polkit util-linux libxkbcommon-devel wayland-devel \
            mesa-libGL-devel libX11-devel libXcursor-devel libXi-devel libXrandr-devel
    elif have pacman; then
        as_root pacman -S --needed --noconfirm base-devel curl git parted dosfstools ntfs-3g exfatprogs \
            e2fsprogs p7zip zenity polkit util-linux libxkbcommon wayland libglvnd libx11 libxcursor libxi libxrandr
    elif have zypper; then
        as_root zypper --non-interactive install gcc pkg-config curl git parted dosfstools ntfs-3g \
            exfatprogs e2fsprogs 7zip zenity polkit util-linux libxkbcommon-devel wayland-devel \
            Mesa-libGL-devel libX11-devel
    else
        warn "unsupported package manager; install parted, dosfstools, ntfs-3g, exfatprogs, p7zip, zenity and polkit manually"
    fi
}

# Succeeds when version $1 >= $2 (dotted numbers).
version_ge() {
    [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n 1)" = "$2" ]
}

ensure_rust() {
    if have cargo && have rustc; then
        current="$(rustc --version | awk '{print $2}')"
        if version_ge "$current" "$MIN_RUST"; then
            return
        fi
        warn "Rust $current is older than $MIN_RUST"
    fi
    if have rustup; then
        say "Updating Rust toolchain"
        rustup update stable
    else
        say "Installing Rust (rustup, minimal profile)"
        curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal
    fi
    # shellcheck disable=SC1091
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
    have cargo || die "cargo is not available after installing Rust"
}

fetch_source() {
    # Use the local checkout when the script is run from inside one.
    here="$(cd "$(dirname "$0")" 2>/dev/null && pwd || true)"
    if [ -n "$here" ] && [ -f "$here/Cargo.toml" ] && [ -f "$here/org.rufux.policy" ]; then
        SRC="$here"
        return
    fi
    have git || die "git is required to download the source"
    WORKDIR="$(mktemp -d)"
    say "Downloading Rufux ($REF)"
    git clone --quiet --depth 1 --branch "$REF" "$REPO_URL" "$WORKDIR/rufux"
    SRC="$WORKDIR/rufux"
}

cleanup() {
    [ -z "${WORKDIR:-}" ] || rm -rf "$WORKDIR"
}

build() {
    say "Building (this takes a few minutes)"
    (cd "$SRC" && cargo build --release --locked)
}

install_files() {
    bindir="$PREFIX/bin"
    say "Installing to $PREFIX"
    as_root install -d "$DESTDIR$bindir" "$DESTDIR$DATADIR/applications" "$DESTDIR$DATADIR/polkit-1/actions"
    as_root install -m 755 "$SRC/target/release/rufux" "$DESTDIR$bindir/rufux"
    as_root install -m 644 "$SRC/rufux.desktop" "$DESTDIR$DATADIR/applications/rufux.desktop"

    # The Polkit action is bound to the absolute path of the installed binary.
    policy_tmp="$(mktemp)"
    sed "s|@BINDIR@|$bindir|g" "$SRC/org.rufux.policy" > "$policy_tmp"
    as_root install -m 644 "$policy_tmp" "$DESTDIR$DATADIR/polkit-1/actions/org.rufux.policy"
    rm -f "$policy_tmp"

    for size in 48 128 256; do
        as_root install -d "$DESTDIR$DATADIR/icons/hicolor/${size}x${size}/apps"
        as_root install -m 644 "$SRC/assets/icons/rufus-$size.png" \
            "$DESTDIR$DATADIR/icons/hicolor/${size}x${size}/apps/rufux.png"
    done

    [ -n "$DESTDIR" ] || {
        have update-desktop-database && as_root update-desktop-database "$DATADIR/applications" 2>/dev/null || true
        have gtk-update-icon-cache && as_root gtk-update-icon-cache -q "$DATADIR/icons/hicolor" 2>/dev/null || true
    }
}

uninstall_files() {
    say "Removing Rufux from $PREFIX"
    as_root rm -f "$DESTDIR$PREFIX/bin/rufux" \
        "$DESTDIR$DATADIR/applications/rufux.desktop" \
        "$DESTDIR$DATADIR/polkit-1/actions/org.rufux.policy"
    for size in 48 128 256; do
        as_root rm -f "$DESTDIR$DATADIR/icons/hicolor/${size}x${size}/apps/rufux.png"
    done
    say "Rufux has been removed"
}

main() {
    parse_args "$@"
    check_platform

    if [ "$UNINSTALL" -eq 1 ]; then
        uninstall_files
        return
    fi

    trap cleanup EXIT INT TERM
    install_packages
    ensure_rust
    fetch_source
    build
    install_files

    say "Done. Start Rufux from your application menu or run: $PREFIX/bin/rufux"
    case ":$PATH:" in *":$PREFIX/bin:"*) ;; *) warn "$PREFIX/bin is not in your PATH" ;; esac
}

# Everything runs from main so a truncated download cannot execute half a script.
main "$@"

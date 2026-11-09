#!/usr/bin/env bash
# RustCraft installer for Linux and macOS.
# Detects the OS, architecture and Linux distribution, then offers
# install, update and uninstall actions through a small TUI.
set -euo pipefail

REPO="jephersonRD/RustCraft"
BIN="rustcraft"
VERSION="${RUSTCRAFT_VERSION:-latest}"
PREFIX="${RUSTCRAFT_PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
XDG_DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
DATA_DIR="${RUSTCRAFT_DATA_DIR:-$XDG_DATA/RustCraft}"
DESKTOP_DIR="$XDG_DATA/applications"
ICON_DIR="$XDG_DATA/icons/hicolor/512x512/apps"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")" && pwd)"

ACTION=""
PURGE=0
ASSUME_YES=0
OS=""
ARCH=""
DISTRO=""
PKG=""
ASSET=""
URL=""

# ---------------------------------------------------------------------------
# Terminal detection and colors
# ---------------------------------------------------------------------------
if [ -t 1 ] && [ -t 2 ]; then
    TTY_OK=1
else
    TTY_OK=0
fi

color_for() {
    # Color variables only when output goes to a real terminal.
    if [ "$TTY_OK" -eq 1 ]; then
        printf '%b' "$1"
    fi
}

RUST_FONT=$'\033[1;38;5;202m'   # blocks: bright orange
RUST_SHADOW=$'\033[2m'          # dimmer shadow characters
YELLOW=$'\033[33m'
ORANGE=$'\033[38;5;208m'
GREEN=$'\033[32m'
RED=$'\033[31m'
GREY=$'\033[90m'
ESC=$'\033'
CYAN=$'\033[36m'
WHITE=$'\033[37m'
BOLD=$'\033[1m'
DIM=$'\033[2m'
RESET=$'\033[0m'
CURSOR_HIDE=$'\033[?25l'
CURSOR_SHOW=$'\033[?25h'
CLEAR_LINE=$'\033[K'

say()  { printf '%s\n' "${BOLD}${YELLOW}RustCraft${RESET} $*"; }
ok()   { printf '%s\n' "${GREEN}✔${RESET} $*"; }
warn() { printf '%s\n' "${YELLOW}!${RESET} $*"; }
die()  { printf '%s\n' "${RED}error: ${RESET}$*" >&2; exit 1; }

restore_cursor() { [ "$TTY_OK" -eq 1 ] && printf '%b' "$CURSOR_SHOW" || true; }
trap 'restore_cursor' EXIT
trap 'restore_cursor; exit 130' INT TERM

center_text() {
    local text="$1" width="#"
    width=$(tput cols 2>/dev/null || printf %s "${#text}")
    [ "$width" -lt "${#text}" ] 2>/dev/null && width=${#text}
    local pad=$(( (width - ${#text}) / 2 ))
    [ "$pad" -lt 0 ] && pad=0
    printf '%*s%s\n' "$pad" '' "$text"
}

# ---------------------------------------------------------------------------
# Banner
# ---------------------------------------------------------------------------
show_banner() {
    if [ "$TTY_OK" -ne 1 ]; then
        printf 'RUSTCRAFT\n\n'
        return
    fi

    clear

    local cols width
    cols=$(tput cols 2>/dev/null || echo 80)
    if [ "$cols" -lt 80 ]; then
        printf '\n'
        center_text "${BOLD}${ORANGE}R U S T C R A F T${RESET}"
        printf '\n\n'
        center_text "${GREY}━━━━━━━━━━━━━━━━${RESET}"
        center_text "${DIM}lanzador · versión ${VERSION}${RESET}"
        printf '\n'
        return
    fi

    local lines=(
        ' ██████╗ ██╗   ██╗███████╗████████╗ ██████╗██████╗  █████╗ ███████╗████████╗'
        ' ██╔══██╗██║   ██║██╔════╝╚══██╔══╝██╔════╝██╔══██╗██╔══██╗██╔════╝╚══██╔══╝'
        ' ██████╔╝██║   ██║███████╗   ██║   ██║     ██████╔╝███████║█████╗     ██║'
        ' ██╔══██╗██║   ██║╚════██║   ██║   ██║     ██╔══██╗██╔══██║██╔══╝     ██║'
        ' ██║  ██║╚██████╔╝███████║   ██║   ╚██████╗██║  ██║██║  ██║██║        ██║'
        ' ╚═╝  ╚═╝ ╚═════╝ ╚══════╝   ╚═╝    ╚═════╝╚═╝  ╚═╝╚═╝  ╚═╝╚═╝        ╚═╝'
    )
    # Rust palette: bright orange fading into rust red.
    local colors=(208 202 196 196 124 88 52)

    local line color shaded
    for line in ${!lines[*]}; do
        color="${colors[$line]:-52}"
        # Block characters keep the vivid gradient color; shadow characters
        # (╗ ╔ ║ ═ ╝ ╚) are dimmed to give the 3D effect.
        shaded=$(sed -e "s/[╗╔║═╝╚]/${ESC}[2m&${ESC}[38;5;${color}m/g" \
                 <<< "${lines[$line]}")
        printf '%b%b%s%b\n' "\033[38;5;${color}m" "$shaded" "" "$RESET"
    done

    printf '\n'
    center_text "${BOLD}${WHITE}lanzador offline para Horizon · versión ${VERSION}${RESET}"
    center_text "${ORANGE}━━━━━━━━━━━━${RESET}"
    printf '\n'
    printf '%s\n' "  ${GREY}${DISTRO:-} · ${ARCH:-}${RESET}"
    [ -n "$PKG" ] && printf '%s\n' "  ${GREY}gestor de paquetes: ${PKG}${RESET}"
    printf '\n'
}

# ---------------------------------------------------------------------------
# Progress bar (4 fixed lines redrawn in place)
#   <nombre>
#   [████████░░░░] 80%
#   800 MB / 1.0 GB
#   12.5 MB/s  •  Remaining: 200 MB
# ---------------------------------------------------------------------------
BAR_WIDTH=32

human_size() {
    local bytes=${1:-0}
    if   [ "$bytes" -ge 1073741824 ]; then awk "BEGIN{printf \"%.1f GB\", $bytes/1073741824}"
    elif [ "$bytes" -ge 1048576 ];    then awk "BEGIN{printf \"%.1f MB\", $bytes/1048576}"
    elif [ "$bytes" -ge 1024 ];       then awk "BEGIN{printf \"%.1f KB\", $bytes/1024}"
    else                                   printf '%s B' "$bytes"
    fi
}

draw_progress() {
    # draw_progress <name> <downloaded> <total> <speed> [percent] [forcer_100]
    local name="$1" done_bytes="$2" total="$3" speed="$4"
    local pct="" force="${6:-0}"
    if [ "$total" -gt 0 ] && [ "$force" -ne 1 ]; then
        pct=$(( done_bytes * 100 / total ))
    elif [ "$force" -eq 1 ]; then
        pct=100
    else
        pct=""
    fi

    printf '%b\r%b\n' "$CLEAR_LINE" "${BOLD}${WHITE}${name}${RESET}"
    if [ -n "$pct" ]; then
        local filled=$(( pct * BAR_WIDTH / 100 ))
        local empty=$(( BAR_WIDTH - filled ))
        local bar="" i=0
        while [ "$i" -lt "$filled" ]; do bar+="█"; i=$((i+1)); done
        i=0
        while [ "$i" -lt "$empty" ]; do bar+="░"; i=$((i+1)); done
        printf '%b\r%b[ %b%b%b %b]%b %s%%\n' "$CLEAR_LINE" "" \
            "${ORANGE}" "$bar" "$RESET" "$GREY" "$GREY" "$pct"
        printf '%b\r%b%s / %s\n' "$CLEAR_LINE" "" "$(human_size "$done_bytes")" "$(human_size "$total")"
        local remain=$(( total - done_bytes )); [ "$remain" -lt 0 ] && remain=0
        printf '%b\r%b%s • Remaining: %s\n' "$CLEAR_LINE" "" "$speed" "$(human_size "$remain")"
    else
        # Unknown size: indeterminate bar cycling with size only.
        local filled=$(( done_bytes % 100 * BAR_WIDTH / 100 ))
        local empty=$(( BAR_WIDTH - filled ))
        local bar="" i=0
        while [ "$i" -lt "$filled" ]; do bar+="█"; i=$((i+1)); done
        i=0
        while [ "$i" -lt "$empty" ]; do bar+="░"; i=$((i+1)); done
        printf '%b\r%b[ %b%s%b ]%b %s\n' "$CLEAR_LINE" "" \
            "${ORANGE}" "$bar" "$RESET" "$GREY" "$(human_size "$done_bytes")"
        printf '\r%b\r%b\n' "$CLEAR_LINE" ""
        printf '\r%b\r%b\n' "$CLEAR_LINE" ""
    fi
    printf '%b\r%b' "$CLEAR_LINE" ""
}

download_with_progress() {
    # Background download: curl/wget writes to $dest while the loop polls its
    # size and repaints the bar. Same retry/timeout flags as before.
    local url="$1" dest="$2"
    local name
    name="${dest##*/}"

    if [ "$TTY_OK" -ne 1 ]; then
        if command -v curl >/dev/null 2>&1; then
            curl -fL --retry 3 --connect-timeout 15 "$url" -o "$dest"
        elif command -v wget >/dev/null 2>&1; then
            wget -q --timeout=30 -O "$dest" "$url"
        else
            die "necesitas curl o wget para descargar RustCraft"
        fi
        return
    fi

    command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1 ||
        die "necesitas curl o wget para descargar RustCraft"

    # Expected size: Content-Length from the redirect chain.
    local total=0
    if command -v curl >/dev/null 2>&1; then
        total=$(curl -fsIL --connect-timeout 15 "$url" 2>/dev/null | grep -i '^content-length:' | tail -1 | tr -dc '0-9') || total=0
        [ -n "$total" ] && [ "$total" -gt 0 ] || total=0
    fi
    # Fallback for release URLs: browsers follow the redirect to a signed S3 URL.
    if [ "$total" -eq 0 ]; then
        total=$(curl -fsIL -o /dev/null -w '%{size_download}' --connect-timeout 15 "$url" 2>/dev/null) || total=0
    fi

    printf '%b' "$CURSOR_HIDE"

    local pid exit_code="1"
    if command -v curl >/dev/null 2>&1; then
        curl -fL --retry 3 --connect-timeout 15 "$url" -o "$dest" >/dev/null 2>&1 &
    else
        wget -q --show-progress --timeout=30 -O "$dest" "$url" >/dev/null 2>&1 &
    fi
    pid=$!

    # Moving-average speed over ~2 s so the numbers do not jitter.
    local last_size=0 last_time now size speed avg=0 samples=0
    local t_start; t_start=$(date +%s%N 2>/dev/null || printf 0)
    local first_draw=1

    while kill -0 "$pid" 2>/dev/null; do
        size=$(stat -c%s "$dest" 2>/dev/null || stat -f%z "$dest" 2>/dev/null || echo 0)
        now=$(date +%s%N 2>/dev/null || printf 0)
        if [ "$first_draw" -eq 0 ] && [ "$now" -gt "$last_time" ] && [ "$size" -ge "$last_size" ]; then
            speed=$(( (size - last_size) * 1000000000 / (now - last_time) ))
            # 2 s moving average
            if [ "$samples" -eq 0 ]; then avg=$speed; else avg=$(( (avg * 3 + speed) / 4 )); fi
            samples=$((samples+1))
        fi
        last_size=$size
        last_time=$now
        first_draw=0
        draw_progress "$name" "$size" "$total" "$([[ "$samples" -gt 0 ]] && printf '%s/s' "$(human_size "$avg")" || printf '%s' "${GREY}…${RESET}")"
        # The block emits 4 lines; walk back up to redraw in place.
        sleep 0.2
        printf '\033[4A'
    done

    size=$(stat -c%s "$dest" 2>/dev/null || stat -f%z "$dest" 2>/dev/null || echo 0)
    wait "$pid" 2>/dev/null || exit_code=0
    # curl -f exits non-zero on HTTP errors; trust the file existence.
    if [ -s "$dest" ]; then
        draw_progress "$name" "$size" "$total" "done" 0 1
        ok "Descarga completada"
        printf '\n'
        return 0
    fi
    return 1
}

download() {
    local url="$1" dest="$2"
    if ! download_with_progress "$url" "$dest"; then
        die "no se pudo descargar $url
  Revisa tu conexión o descárgalo a mano desde https://github.com/$REPO/releases"
    fi
}

# ---------------------------------------------------------------------------
# Spinner for post-download stages
# ---------------------------------------------------------------------------
spinner() {
    # spinner <desc> <command...> : run command, animate while it runs.
    local desc="$1"; shift
    if [ "$TTY_OK" -ne 1 ]; then
        say "$desc"
        "$@"
        local status=$?
        return $status
    fi
    printf '%b' "$CURSOR_HIDE"
    "$@" &
    local pid=$!
    local frames=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
    local i=0
    printf '\r%b %s ' "$CLEAR_LINE" "$desc"
    while kill -0 "$pid" 2>/dev/null; do
        printf '\r%b %s %s %b' "$CLEAR_LINE" "${ORANGE}${frames[$i]}${RESET}" "$desc" ""
        i=$(( (i + 1) % 10 ))
        sleep 0.08
    done
    wait "$pid"
    local status=$?
    if [ "$status" -eq 0 ]; then
        printf '\r%b %b %s%s ✔ %b\n' "$CLEAR_LINE" "${GREEN}✔${RESET}" "$desc" " ${GREY}hecho${RESET}" "$RESET"
    else
        printf '\r%b %b %s ✖ (código %s)\n' "$CLEAR_LINE" "${RED}✖${RESET}" "$desc" "$status"
    fi
    return $status
}

# ---------------------------------------------------------------------------
# Detection and install logic (unchanged)
# ---------------------------------------------------------------------------
usage() {
    cat <<EOF
RustCraft installer

Uso:
  ./install.sh [install|update|uninstall] [opciones]

Acciones:
  install       Instala la última versión (o la indicada en RUSTCRAFT_VERSION).
  update        Descarga e instala la última versión disponible.
  uninstall     Elimina el binario. Pregunta antes de borrar los datos.

Opciones:
  --purge       Al desinstalar, borra también los datos y las instancias.
  -y, --yes     No pregunta (los datos solo se borran con --purge).
  -h, --help    Muestra esta ayuda.

Variables:
  RUSTCRAFT_VERSION   Etiqueta del release, por ejemplo v0.1.1. Por defecto: latest.
  RUSTCRAFT_PREFIX    Prefijo de instalación. Por defecto: ~/.local.
  RUSTCRAFT_DATA_DIR  Carpeta de datos. Por defecto: ~/.local/share/RustCraft.
EOF
}

detect_os() {
    case "$(uname -s)" in
        Linux*) OS="linux" ;;
        Darwin*) OS="macos" ;;
        MINGW*|MSYS*|CYGWIN*) OS="windows" ;;
        *) die "sistema no soportado: $(uname -s)" ;;
    esac
}

delegate_to_windows() {
    local ps1="$SCRIPT_DIR/install.ps1"
    if [ ! -f "$ps1" ]; then
        command -v curl >/dev/null 2>&1 || die "necesitas curl para descargar install.ps1"
        ps1="$(mktemp).ps1"
        download "https://raw.githubusercontent.com/$REPO/main/install.ps1" "$ps1"
    fi
    command -v powershell.exe >/dev/null 2>&1 ||
        die "abre PowerShell y ejecuta install.ps1 directamente"
    exec powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$ps1" "$@"
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64) ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        armv7l|armv6l) ARCH="armv7" ;;
        *) ARCH="$(uname -m)" ;;
    esac
}

detect_distro() {
    if [ "$OS" = "linux" ]; then
        if [ -r /etc/os-release ]; then
            # shellcheck disable=SC1091
            . /etc/os-release
            DISTRO="${PRETTY_NAME:-${NAME:-Linux}}"
        else
            DISTRO="Linux"
        fi
        for candidate in apt-get dnf yum pacman zypper apk xbps-install eopkg; do
            if command -v "$candidate" >/dev/null 2>&1; then
                PKG="$candidate"
                break
            fi
        done
    else
        DISTRO="$(uname -s)"
    fi
}

is_installed() {
    [ -x "$BIN_DIR/$BIN" ]
}

resolve_asset() {
    case "$OS-$ARCH" in
        linux-x86_64) ASSET="rustcraft-linux-x86_64" ;;
        linux-aarch64) ASSET="rustcraft-linux-aarch64" ;;
        macos-x86_64) ASSET="rustcraft-macos-x86_64" ;;
        macos-aarch64) ASSET="rustcraft-macos-arm64" ;;
        *) ASSET="" ;;
    esac

    if [ -n "$ASSET" ]; then
        if [ "$VERSION" = "latest" ]; then
            URL="https://github.com/$REPO/releases/latest/download/$ASSET"
        else
            URL="https://github.com/$REPO/releases/download/$VERSION/$ASSET"
        fi
    fi
}

build_from_source() {
    command -v cargo >/dev/null 2>&1 ||
        die "no hay binario para $OS/$ARCH y falta Cargo. Instala Rust desde https://rustup.rs"
    say "compilando desde el código fuente (puede tardar unos minutos)…"
    cargo install --git "https://github.com/$REPO" --locked --root "$PREFIX" --force
}

ensure_path() {
    case ":$PATH:" in
        *":$BIN_DIR:"*) return ;;
    esac
    export PATH="$BIN_DIR:$PATH"
    local line
    line="export PATH=\"$BIN_DIR:\$PATH\""
    local touched=0
    local rc
    for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
        [ -f "$rc" ] || continue
        touched=1
        if ! grep -Fq "$line" "$rc"; then
            printf '\n# RustCraft\n%s\n' "$line" >> "$rc"
        fi
    done
    if [ "$touched" -eq 0 ]; then
        printf '\n# RustCraft\n%s\n' "$line" >> "$HOME/.profile"
    fi
    warn "se añadió $BIN_DIR al PATH; abre una terminal nueva si no aparece"
}

java_major() {
    command -v java >/dev/null 2>&1 || return 0
    java -version 2>&1 | sed -n '1s/.*version "\([0-9][0-9]*\).*/\1/p'
}

java_package() {
    case "$PKG" in
        apt-get) printf '%s' "openjdk-25-jre-headless" ;;
        dnf|yum) printf '%s' "java-25-openjdk-headless" ;;
        pacman) printf '%s' "jre-openjdk-headless" ;;
        zypper) printf '%s' "java-25-openjdk-headless" ;;
        apk) printf '%s' "openjdk25-jre-headless" ;;
        xbps-install) printf '%s' "openjdk25-jre" ;;
        eopkg) printf '%s' "openjdk-25-jre" ;;
        *) printf '%s' "" ;;
    esac
}

java_hint() {
    local package
    package="$(java_package)"
    if [ -z "$package" ]; then
        printf '%s' "instala Temurin 25 desde https://adoptium.net"
        return
    fi
    case "$PKG" in
        apt-get) printf '%s' "sudo apt-get update && sudo apt-get install -y $package" ;;
        dnf|yum|zypper) printf '%s' "sudo $PKG install -y $package" ;;
        pacman) printf '%s' "sudo pacman -S --needed $package" ;;
        apk) printf '%s' "sudo apk add $package" ;;
        xbps-install) printf '%s' "sudo xbps-install -S $package" ;;
        eopkg) printf '%s' "sudo eopkg install $package" ;;
    esac
}

install_java() {
    local major
    major="$(java_major || true)"
    if [ -n "$major" ] && [ "$major" -ge 25 ] 2>/dev/null; then
        ok "Java $major detectado"
        return
    fi

    warn "Java 25 o superior no está instalado (lo necesita horizon-prepare)"
    if [ "$OS" != "linux" ]; then
        printf '  Instala Temurin 25 desde https://adoptium.net\n'
        return
    fi

    printf '  Sistema: %s\n' "$DISTRO"
    [ -n "$PKG" ] && printf '  Gestor:  %s\n' "$PKG"
    printf '  Comando: %s\n' "$(java_hint)"

    local package
    package="$(java_package)"
    if [ -z "$PKG" ] || [ -z "$package" ]; then
        return
    fi

    if [ "$ASSUME_YES" -eq 0 ]; then
        if [ ! -t 0 ]; then
            warn "sin terminal interactiva; ejecuta el comando de arriba para instalar Java"
            return
        fi
        local answer=""
        printf '¿Instalar Java 25 ahora con %s? [S/n] ' "$PKG"
        read -r answer || true
        case "$answer" in
            n|N|no|No) warn "Java no instalado; RustCraft no podrá preparar Horizon"; return ;;
        esac
    fi

    say "instalando Java 25…"
    local installed=0
    case "$PKG" in
        apt-get)
            sudo apt-get update && sudo apt-get install -y "$package" && installed=1
            ;;
        dnf|yum|zypper)
            sudo "$PKG" install -y "$package" && installed=1
            ;;
        pacman)
            sudo pacman -S --needed --noconfirm "$package" && installed=1
            ;;
        apk)
            sudo apk add "$package" && installed=1
            ;;
        xbps-install)
            sudo xbps-install -S -y "$package" && installed=1
            ;;
        eopkg)
            sudo eopkg install -y "$package" && installed=1
            ;;
    esac || true

    major="$(java_major || true)"
    if [ "$installed" -eq 1 ] && [ -n "$major" ] && [ "$major" -ge 25 ] 2>/dev/null; then
        ok "Java $major instalado"
        return
    fi
    warn "no se pudo instalar Java automáticamente"
    printf '  Ejecuta: %s\n' "$(java_hint)"
    printf '  O instala Temurin 25 desde https://adoptium.net\n'
}

install_desktop_entry() {
    [ "$OS" = "linux" ] || return 0

    mkdir -p "$DESKTOP_DIR" || return 1

    local icon_src=""
    local icon_tmp=""
    if [ -f "$SCRIPT_DIR/RustCraft.png" ]; then
        icon_src="$SCRIPT_DIR/RustCraft.png"
    else
        icon_tmp="$(mktemp)"
        if download "https://raw.githubusercontent.com/$REPO/main/RustCraft.png" "$icon_tmp" >/dev/null 2>&1; then
            icon_src="$icon_tmp"
        else
            rm -f "$icon_tmp"
            icon_tmp=""
        fi
    fi

    if [ -n "$icon_src" ]; then
        mkdir -p "$ICON_DIR" || return 1
        install -m 644 "$icon_src" "$ICON_DIR/rustcraft.png" || return 1
        if [ -n "$icon_tmp" ]; then
            rm -f "$icon_tmp"
        fi
        if command -v gtk-update-icon-cache >/dev/null 2>&1; then
            gtk-update-icon-cache -f -t "$XDG_DATA/icons/hicolor" >/dev/null 2>&1 || true
        fi
    else
        warn "no se pudo descargar el icono; se usará uno genérico"
    fi

    local exec_path="$BIN_DIR/$BIN"
    exec_path="${exec_path//\\/\\\\}"
    exec_path="${exec_path//\"/\\\"}"
    exec_path="${exec_path//%/%%}"

    cat > "$DESKTOP_DIR/rustcraft.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=RustCraft
GenericName=Lanzador de Minecraft
Comment=Lanzador offline para Horizon
Exec="$exec_path"
TryExec=$exec_path
Icon=${icon_src:+$ICON_DIR/rustcraft.png}
Terminal=false
Categories=Game;ActionGame;
Keywords=minecraft;horizon;launcher;
StartupNotify=true
StartupWMClass=RustCraft
EOF
    if [ -z "$icon_src" ]; then
        sed -i 's|^Icon=.*|Icon=rustcraft|' "$DESKTOP_DIR/rustcraft.desktop"
    fi
    chmod 644 "$DESKTOP_DIR/rustcraft.desktop"

    # Some desktops only scan the default applications directory.
    local default_dir="$HOME/.local/share/applications"
    if [ "$DESKTOP_DIR" != "$default_dir" ]; then
        mkdir -p "$default_dir" 2>/dev/null || true
        cp -f "$DESKTOP_DIR/rustcraft.desktop" "$default_dir/rustcraft.desktop" 2>/dev/null || true
    fi

    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true
        [ "$DESKTOP_DIR" = "$default_dir" ] ||
            update-desktop-database "$default_dir" >/dev/null 2>&1 || true
    fi
    if command -v xdg-desktop-menu >/dev/null 2>&1; then
        xdg-desktop-menu forceupdate >/dev/null 2>&1 || true
    fi
    ok "entrada añadida al menú de aplicaciones"
}

remove_desktop_entry() {
    [ "$OS" = "linux" ] || return 0

    local removed=0
    if [ -f "$DESKTOP_DIR/rustcraft.desktop" ]; then
        rm -f "$DESKTOP_DIR/rustcraft.desktop"
        removed=1
    fi
    local default_entry="$HOME/.local/share/applications/rustcraft.desktop"
    if [ -f "$default_entry" ] && [ "$default_entry" != "$DESKTOP_DIR/rustcraft.desktop" ]; then
        rm -f "$default_entry"
        removed=1
    fi
    if [ -f "$ICON_DIR/rustcraft.png" ]; then
        rm -f "$ICON_DIR/rustcraft.png"
        removed=1
    fi

    if [ "$removed" -eq 1 ]; then
        if command -v update-desktop-database >/dev/null 2>&1; then
            update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true
        fi
        if command -v gtk-update-icon-cache >/dev/null 2>&1; then
            gtk-update-icon-cache -f -t "$XDG_DATA/icons/hicolor" >/dev/null 2>&1 || true
        fi
        ok "entrada del menú de aplicaciones eliminada"
    fi
}

# ---------------------------------------------------------------------------
# Summary box
# ---------------------------------------------------------------------------
summary_box() {
    local title="$1" path="$2"
    local top="╭──────────────────────────────────────────────────╮"
    local bot="╰──────────────────────────────────────────────────╯"
    printf '\n%s\n' "${ORANGE}${top}${RESET}"
    printf '%b  %b%s%b\n' "$ORANGE" "$BOLD${WHITE}" "$title" "$RESET"
    printf '%b  %b%s%b\n' "$ORANGE" "$GREY" "instalación: $path" "$RESET"
    [ -n "$(java_major 2>/dev/null || true)" ] && {
        local jm; jm="$(java_major 2>/dev/null || true)"
        printf '%b  %b%s%b\n' "$ORANGE" "$GREY" "Java: $jm" "$RESET"
    }
    printf '%s\n' "${ORANGE}${bot}${RESET}"
    printf '\n  Ejecuta: %brustcraft%b\n' "$BOLD${ORANGE}" "$RESET"
    printf '\n'
}

# ---------------------------------------------------------------------------
# High level actions: install() update() uninstall()
# ---------------------------------------------------------------------------
install() {
    printf '\n'
    say "descargando binario para $OS/$ARCH…"
    printf '\n'

    mkdir -p "$BIN_DIR"
    if [ -n "$ASSET" ]; then
        local tmp
        tmp="$(mktemp)"
        local tmp_named
        tmp_named="${TMPDIR:-/tmp}/$(basename "${ASSET}").$$"
        mv "$tmp" "$tmp_named" 2>/dev/null || tmp_named="$tmp"
        if download "$URL" "$tmp_named"; then
            install -m 755 "$tmp_named" "$BIN_DIR/$BIN"
            rm -f "$tmp_named"
        else
            rm -f "$tmp_named"
            warn "no hay binario precompilado para $OS/$ARCH"
            build_from_source
        fi
    else
        warn "plataforma sin binario precompilado: $OS/$ARCH"
        build_from_source
    fi

    spinner "Configurando launcher..." true
    ensure_path
    if spinner "Creando acceso directo..." install_desktop_entry; then
        :
    else
        warn "no se pudo crear la entrada del menú de aplicaciones"
    fi
    ok "RustCraft instalado en $BIN_DIR/$BIN"
    summary_box "RustCraft instalado correctamente" "$BIN_DIR/$BIN"
    install_java
    printf '\n'
}

update() {
    printf '\n'
    say "buscando la última versión (${VERSION})…"
    install
}

uninstall() {
    printf '\n'
    if [ "$ASSUME_YES" -eq 0 ] && [ -t 0 ]; then
        local answer=""
        printf '¿Desinstalar RustCraft de %s? [s/N] ' "$BIN_DIR"
        read -r answer || true
        case "$answer" in
            s|S|si|sí|y|Y) ;;
            *) warn "desinstalación cancelada"; return ;;
        esac
    fi

    remove_desktop_entry

    if [ -f "$BIN_DIR/$BIN" ]; then
        rm -f "$BIN_DIR/$BIN"
        ok "binario eliminado de $BIN_DIR/$BIN"
    else
        warn "RustCraft no estaba instalado en $BIN_DIR"
    fi

    local delete_data=0
    if [ "$PURGE" -eq 1 ]; then
        delete_data=1
    elif [ "$ASSUME_YES" -eq 0 ] && [ -t 0 ] && [ -d "$DATA_DIR" ]; then
        local answer=""
        printf '¿Eliminar también los datos y las instancias en %s? [s/N] ' "$DATA_DIR"
        read -r answer || true
        case "$answer" in
            s|S|si|sí|y|Y) delete_data=1 ;;
        esac
    fi

    if [ "$delete_data" -eq 1 ] && [ -d "$DATA_DIR" ]; then
        rm -rf "$DATA_DIR"
        ok "datos eliminados de $DATA_DIR"
    else
        printf 'Datos conservados en %s\n' "$DATA_DIR"
    fi

    local line
    line="export PATH=\"$BIN_DIR:\$PATH\""
    local rc
    for rc in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
        [ -f "$rc" ] || continue
        if grep -Fq "$line" "$rc"; then
            grep -Fv "$line" "$rc" > "$rc.rustcraft.tmp" || true
            mv "$rc.rustcraft.tmp" "$rc"
        fi
    done
    ok "RustCraft desinstalado"
    printf '\n'
}

# ---------------------------------------------------------------------------
# Menu (single keypress) and entry point
# ---------------------------------------------------------------------------
show_menu() {
    local installed="" already=0
    already=0
    is_installed && already=1

    if [ "$TTY_OK" -eq 1 ]; then
        clear
    fi
    show_banner

    local O="${ORANGE}" G="${GREY}" B="${BOLD}" R="${RESET}"
    if [ "$already" -eq 1 ]; then
        printf '%s\n' "${GREEN}»${RESET} RustCraft ya está instalado en ${WHITE}$BIN_DIR/$BIN${RESET}"
    else
        printf '%s\n' "${GREY}RustCraft no está instalado todavía${RESET}"
    fi
    printf '\n'
    printf '%s\n' "${ORANGE}╭─────────────────────────────────────╮${RESET}"

    local item_1 item_2 item_3 item_4
    if [ "$already" -eq 1 ]; then
        item_1="${GREY}[1] ⬇  Descargar / Instalar${RESET}  ${GREY}(ya instalado)${RESET}"
        item_2="${WHITE}[2] ${O}⟳${RESET}  Actualizar${RESET}"
        item_3="${WHITE}[3] ${O}✖${RESET}  Desinstalar${RESET}"
        item_4="${WHITE}[0] ${O}⏻${RESET}  Salir${RESET}"
    else
        item_1="${WHITE}[1] ${O}⬇${RESET}  Descargar / Instalar${RESET}"
        item_2="${GREY}[2] ⟳  Actualizar${RESET}  ${GREY}(no instalado)${RESET}"
        item_3="${GREY}[3] ✖  Desinstalar${RESET}  ${GREY}(no instalado)${RESET}"
        item_4="${WHITE}[0] ${O}⏻${RESET}  Salir${RESET}"
    fi
    printf '%b %b\n' "${ORANGE}│${RESET}" "  $item_1"
    printf '%b %b\n' "${ORANGE}│${RESET}" "  $item_2"
    printf '%b %b\n' "${ORANGE}│${RESET}" "  $item_3"
    printf '%b %b\n' "${ORANGE}│${RESET}" "  $item_4"
    printf '%s\n' "${ORANGE}╰─────────────────────────────────────╯${RESET}"
    printf '\n'
    printf '%s' "  ${BOLD}${WHITE}Pulsa una tecla:${RESET} "

    local key=""
    if [ "$TTY_OK" -eq 1 ]; then
        IFS='' read -rsn1 key </dev/tty 2>/dev/null || key=""
    else
        printf '\n'
        read -r key </dev/tty 2>/dev/null || read -r key || key=""
    fi
    printf '%s\n' "$key"
    case "$key" in
        1)
            if [ "$already" -eq 1 ]; then
                warn "RustCraft ya está instalado; usa Actualizar para actualizar"
            else
                ACTION="install"
            fi
            ;;
        2)
            if [ "$already" -eq 1 ]; then
                ACTION="update"
            else
                warn "RustCraft no está instalado aún; elige Descargar primero"
            fi
            ;;
        3)
            if [ "$already" -eq 1 ]; then
                ACTION="uninstall"
            else
                warn "RustCraft no está instalado; nada que desinstalar"
            fi
            ;;
        0|q|Q)
            printf '%s\n' "${GREY}Hasta pronto.${RESET}"
            exit 0
            ;;
        *)
            die "opción no válida: ${key:-vacía}"
            ;;
    esac
}

parse_args() {
    local arg
    for arg in "$@"; do
        case "$arg" in
            install|--install|-i) ACTION="install" ;;
            update|--update|-u) ACTION="update" ;;
            uninstall|--uninstall|remove|--remove|-r) ACTION="uninstall" ;;
            --purge) PURGE=1 ;;
            -y|--yes) ASSUME_YES=1 ;;
            -h|--help) usage; exit 0 ;;
            *) die "opción desconocida: $arg (usa --help)" ;;
        esac
    done
}

detect_os
if [ "$OS" = "windows" ]; then
    delegate_to_windows "$@"
fi

parse_args "$@"
detect_arch
detect_distro
resolve_asset

if [ -z "$ACTION" ]; then
    if [ ! -t 0 ]; then
        usage
        die "sin terminal interactiva; indica install, update o uninstall"
    fi
    # Interactive loop: after each action you can return to the menu.
    while :; do
        show_menu
        [ -n "$ACTION" ] || continue
        case "$ACTION" in
            install) install ;;
            update) update ;;
            uninstall) uninstall ;;
        esac
        ACTION=""
        local_answer=""
        printf '¿Volver al menú? [s/N] '
        read -r local_answer </dev/tty 2>/dev/null || read -r local_answer || true
        case "$local_answer" in
            s|S|si|sí|y|Y) ;;
            *) break ;;
        esac
    done
    exit 0
fi

show_banner

case "$ACTION" in
    install) install ;;
    update) update ;;
    uninstall) uninstall ;;
    *) die "acción no válida: $ACTION" ;;
esac

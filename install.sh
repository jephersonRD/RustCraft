#!/usr/bin/env bash
# RustCraft installer for Linux and macOS.
# Detects the OS, architecture and Linux distribution, then offers
# install, update and uninstall actions.
set -euo pipefail

REPO="jephersonRD/RustCraft"
BIN="rustcraft"
VERSION="${RUSTCRAFT_VERSION:-latest}"
PREFIX="${RUSTCRAFT_PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
DATA_DIR="${RUSTCRAFT_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/RustCraft}"
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

if [ -t 1 ]; then
    YELLOW=$'\033[33m'
    GREEN=$'\033[32m'
    RED=$'\033[31m'
    BOLD=$'\033[1m'
    RESET=$'\033[0m'
else
    YELLOW=""
    GREEN=""
    RED=""
    BOLD=""
    RESET=""
fi

say()  { printf '%s\n' "${BOLD}${YELLOW}RustCraft${RESET} $*"; }
ok()   { printf '%s\n' "${GREEN}✓${RESET} $*"; }
warn() { printf '%s\n' "${YELLOW}!${RESET} $*"; }
die()  { printf '%s\n' "${RED}error:${RESET} $*" >&2; exit 1; }

usage() {
    cat <<EOF
RustCraft ${VERSION} installer

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
  RUSTCRAFT_VERSION   Etiqueta del release, por ejemplo v0.1.0. Por defecto: latest.
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
        curl -fsSL "https://raw.githubusercontent.com/$REPO/main/install.ps1" -o "$ps1"
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

download() {
    local url="$1" dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fL --retry 3 --connect-timeout 15 --progress-bar "$url" -o "$dest"
    elif command -v wget >/dev/null 2>&1; then
        wget -q --show-progress --timeout=30 -O "$dest" "$url"
    else
        die "necesitas curl o wget para descargar RustCraft"
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

java_hint() {
    case "$PKG" in
        apt-get) printf '%s' "sudo apt-get update && sudo apt-get install -y openjdk-25-jre-headless" ;;
        dnf) printf '%s' "sudo dnf install -y java-25-openjdk-headless" ;;
        yum) printf '%s' "sudo yum install -y java-25-openjdk-headless" ;;
        pacman) printf '%s' "sudo pacman -S --needed jre-openjdk-headless" ;;
        zypper) printf '%s' "sudo zypper install -y java-25-openjdk-headless" ;;
        apk) printf '%s' "sudo apk add openjdk25-jre-headless" ;;
        xbps-install) printf '%s' "sudo xbps-install -S openjdk25-jre" ;;
        eopkg) printf '%s' "sudo eopkg install openjdk-25-jre" ;;
        *) printf '%s' "instala Temurin 25 desde https://adoptium.net" ;;
    esac
}

check_java() {
    local major
    major="$(java_major || true)"
    if [ -n "$major" ] && [ "$major" -ge 25 ] 2>/dev/null; then
        ok "Java $major detectado"
        return
    fi
    warn "Java 25 no está instalado (lo necesita horizon-prepare)"
    if [ "$OS" = "linux" ]; then
        printf '  Sistema: %s\n' "$DISTRO"
        if [ -n "$PKG" ]; then
            printf '  Gestor:  %s\n' "$PKG"
        fi
        printf '  Comando: %s\n' "$(java_hint)"
    else
        printf '  Instala Temurin 25 desde https://adoptium.net\n'
    fi
}

install_app() {
    mkdir -p "$BIN_DIR"
    if [ -n "$ASSET" ]; then
        local tmp
        tmp="$(mktemp)"
        if download "$URL" "$tmp"; then
            install -m 755 "$tmp" "$BIN_DIR/$BIN"
            rm -f "$tmp"
        else
            rm -f "$tmp"
            warn "no hay binario precompilado para $OS/$ARCH"
            build_from_source
        fi
    else
        warn "plataforma sin binario precompilado: $OS/$ARCH"
        build_from_source
    fi
    ensure_path
    ok "RustCraft instalado en $BIN_DIR/$BIN"
    check_java
    printf '\nEjecuta: %srustcraft%s\n' "$YELLOW" "$RESET"
}

update_app() {
    say "buscando la última versión (${VERSION})…"
    install_app
}

uninstall_app() {
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
}

show_menu() {
    printf '%s\n' "${BOLD}RustCraft${RESET} · instalador"
    printf 'Sistema: %s · %s\n' "$DISTRO" "$ARCH"
    if [ -n "$PKG" ]; then
        printf 'Paquetes: %s\n' "$PKG"
    fi
    printf '\n'
    printf '  1) Instalar\n'
    printf '  2) Actualizar\n'
    printf '  3) Desinstalar\n'
    printf '  4) Salir\n\n'
    local option=""
    printf 'Elige una opción [1-4]: '
    read -r option || true
    case "$option" in
        1) ACTION="install" ;;
        2) ACTION="update" ;;
        3) ACTION="uninstall" ;;
        4) exit 0 ;;
        *) die "opción no válida: ${option:-vacía}" ;;
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
    show_menu
fi

case "$ACTION" in
    install) install_app ;;
    update) update_app ;;
    uninstall) uninstall_app ;;
    *) die "acción no válida: $ACTION" ;;
esac

#!/usr/bin/env bash
# install.sh — bootstrap the computer-use-linux MCP server on a Linux box.
#
# This script takes a fresh checkout from `git clone` to a working
# `computer-use-linux mcp` binary in PATH plus the system packages it calls
# (AT-SPI, xdotool). The server drives a virtual X11 display (Xvfb + i3).
#
# Each step is idempotent and individually skippable via flags.
# Re-running the script on a fully provisioned host should print all-green
# and exit 0 without changing anything.

set -euo pipefail
IFS=$'\n\t'

# -----------------------------------------------------------------------------
# Globals & helpers
# -----------------------------------------------------------------------------

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
OS_RELEASE_FILE="${COMPUTER_USE_LINUX_OS_RELEASE_FILE:-/etc/os-release}"
BIN_NAME="computer-use-linux"
INSTALL_DIR="${HOME}/.local/bin"
INSTALL_PATH="${INSTALL_DIR}/${BIN_NAME}"

# Color helpers (degrade gracefully when not on a tty).
if [[ -t 1 ]] && command -v tput >/dev/null 2>&1 && [[ "$(tput colors 2>/dev/null || echo 0)" -ge 8 ]]; then
    C_GREEN="$(tput setaf 2)"
    C_YELLOW="$(tput setaf 3)"
    C_RED="$(tput setaf 1)"
    C_BLUE="$(tput setaf 4)"
    C_BOLD="$(tput bold)"
    C_RESET="$(tput sgr0)"
else
    C_GREEN=""; C_YELLOW=""; C_RED=""; C_BLUE=""; C_BOLD=""; C_RESET=""
fi

log_section() { printf '\n%s==>%s %s%s%s\n' "${C_BLUE}" "${C_RESET}" "${C_BOLD}" "$*" "${C_RESET}"; }
log_ok()      { printf '  %sOK%s   %s\n' "${C_GREEN}" "${C_RESET}" "$*"; }
log_warn()    { printf '  %sWARN%s %s\n' "${C_YELLOW}" "${C_RESET}" "$*"; }
log_skip()    { printf '  %sSKIP%s %s\n' "${C_YELLOW}" "${C_RESET}" "$*"; }
log_fail()    { printf '  %sFAIL%s %s\n' "${C_RED}" "${C_RESET}" "$*"; }
log_info()    { printf '       %s\n' "$*"; }

die() { log_fail "$*"; exit 1; }

# Track failed checks for a final summary.
FAILED_CHECKS=()
record_failure() { FAILED_CHECKS+=("$1"); }

trap 'rc=$?; if [[ $rc -ne 0 ]]; then printf "\n%sinstall.sh aborted (exit %d)%s\n" "${C_RED}" "$rc" "${C_RESET}"; fi' EXIT

# -----------------------------------------------------------------------------
# CLI parsing
# -----------------------------------------------------------------------------

SKIP_SYSTEM_DEPS=0
SKIP_RUST=0
SKIP_BUILD=0
SKIP_DOCTOR=0
FORCE_UNKNOWN_DISTRO=0
PACKAGE_MANAGER_OVERRIDE=""

usage() {
    cat <<EOF
${C_BOLD}install.sh${C_RESET} — provision computer-use-linux on this machine.

Usage: ./install.sh [flags]

Steps (run in order, each idempotent):
  1. Detect distro + X display
  2. Install system packages (apt/dnf/pacman)
  3. Install rustup toolchain
  4. cargo build --release  →  ~/.local/bin/${BIN_NAME}
  5. Run \`${BIN_NAME} doctor\` and report readiness

Flags:
  --skip-system-deps      skip apt/dnf/pacman package install
  --skip-rust             skip rustup install
  --skip-build            skip cargo build (assumes target/release/${BIN_NAME} exists)
  --skip-doctor           skip the final readiness check
  --force-unknown-distro  use the one supported package manager found on PATH
  --package-manager NAME  force apt, dnf, or pacman for system packages
  -h, --help              show this help and exit
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-system-deps)     SKIP_SYSTEM_DEPS=1 ;;
        --skip-rust)            SKIP_RUST=1 ;;
        --skip-build)           SKIP_BUILD=1 ;;
        --skip-doctor)          SKIP_DOCTOR=1 ;;
        --force-unknown-distro) FORCE_UNKNOWN_DISTRO=1 ;;
        --package-manager)
            [[ $# -ge 2 ]] || die "--package-manager requires apt, dnf, or pacman"
            PACKAGE_MANAGER_OVERRIDE="$2"
            shift
            ;;
        --package-manager=*)    PACKAGE_MANAGER_OVERRIDE="${1#*=}" ;;
        -h|--help)              usage; exit 0 ;;
        *)                      usage; die "unknown flag: $1" ;;
    esac
    shift
done

case "${PACKAGE_MANAGER_OVERRIDE}" in
    ""|apt|dnf|pacman) ;;
    *) die "unsupported package manager '${PACKAGE_MANAGER_OVERRIDE}' (expected apt, dnf, or pacman)" ;;
esac

# -----------------------------------------------------------------------------
# Step 1: distro + X display detection
# -----------------------------------------------------------------------------

DISTRO_FAMILY=""
PKG_MANAGER=""

set_package_manager() {
    case "$1" in
        apt)    DISTRO_FAMILY="debian"; PKG_MANAGER="apt" ;;
        dnf)    DISTRO_FAMILY="fedora"; PKG_MANAGER="dnf" ;;
        pacman) DISTRO_FAMILY="arch"; PKG_MANAGER="pacman" ;;
        *) return 1 ;;
    esac
}

package_manager_available() {
    case "$1" in
        apt)    command -v apt-get >/dev/null 2>&1 ;;
        dnf)    command -v dnf >/dev/null 2>&1 ;;
        pacman) command -v pacman >/dev/null 2>&1 ;;
        *) return 1 ;;
    esac
}

available_package_managers() {
    local manager
    for manager in apt dnf pacman; do
        if package_manager_available "${manager}"; then
            printf '%s\n' "${manager}"
        fi
    done
}

detect_distro() {
    log_section "Step 1/6 — detect environment"

    if [[ "$(uname -s)" != "Linux" ]]; then
        die "this script only supports Linux (got $(uname -s)). macOS/*BSD are not supported."
    fi

    if [[ ! -r "${OS_RELEASE_FILE}" ]]; then
        die "${OS_RELEASE_FILE} missing — cannot detect distro."
    fi

    local ID="" ID_LIKE="" PRETTY_NAME=""
    # shellcheck disable=SC1091
    . "${OS_RELEASE_FILE}"
    local id_like="${ID_LIKE:-} ${ID:-}"

    if [[ -n "${PACKAGE_MANAGER_OVERRIDE}" ]]; then
        set_package_manager "${PACKAGE_MANAGER_OVERRIDE}"
        log_warn "using requested package manager: ${PACKAGE_MANAGER_OVERRIDE}"
    else
        case " ${id_like} " in
            *" debian "*|*" ubuntu "*)
                set_package_manager apt ;;
            *" fedora "*|*" rhel "*|*" centos "*)
                set_package_manager dnf ;;
            *" arch "*|*" archlinux "*|*" manjaro "*|*" endeavouros "*|*" artix "*|*" artixlinux "*)
                set_package_manager pacman ;;
            *)
                if [[ ${FORCE_UNKNOWN_DISTRO} -eq 1 ]]; then
                    if [[ ${SKIP_SYSTEM_DEPS} -eq 1 ]]; then
                        DISTRO_FAMILY="unknown"
                        log_warn "unknown distro '${ID:-?}' — system package installation is skipped"
                    else
                        local available=()
                        mapfile -t available < <(available_package_managers)
                        if [[ ${#available[@]} -ne 1 ]]; then
                            log_fail "cannot choose a package manager for '${ID:-unknown}'"
                            log_info "found: ${available[*]:-none}; pass --package-manager apt|dnf|pacman"
                            return 1
                        fi
                        set_package_manager "${available[0]}"
                        log_warn "unknown distro '${ID:-?}' — using detected ${PKG_MANAGER}"
                    fi
                else
                    log_fail "unsupported distro: ${ID:-unknown} (${PRETTY_NAME:-?})"
                    log_info "supported families: debian/ubuntu, fedora, arch/artix"
                    log_info "re-run with --force-unknown-distro or --package-manager apt|dnf|pacman"
                    return 1
                fi ;;
        esac
    fi

    if [[ ${SKIP_SYSTEM_DEPS} -eq 0 ]] && ! package_manager_available "${PKG_MANAGER}"; then
        log_fail "${PKG_MANAGER} was selected but its command is not on PATH"
        return 1
    fi
    if [[ -n "${PKG_MANAGER}" ]]; then
        log_ok "distro family: ${DISTRO_FAMILY} (pkg manager: ${PKG_MANAGER})"
    else
        log_ok "distro family: ${DISTRO_FAMILY} (system packages skipped)"
    fi

    if [[ -n "${DISPLAY:-}" ]]; then
        log_ok "X display: ${DISPLAY}"
    else
        log_warn "DISPLAY is not set; export DISPLAY=:N for the Xvfb display before running the server"
    fi
}

# -----------------------------------------------------------------------------
# Step 2: system package install
# -----------------------------------------------------------------------------

install_system_deps() {
    log_section "Step 2/6 — system packages"
    if [[ ${SKIP_SYSTEM_DEPS} -eq 1 ]]; then log_skip "--skip-system-deps"; return 0; fi

    case "${PKG_MANAGER}" in
        apt)
            local pkgs=(build-essential pkg-config libdbus-1-dev libssl-dev curl at-spi2-core xdotool)
            sudo apt-get update -qq
            log_info "sudo apt-get install -y ${pkgs[*]}"
            sudo apt-get install -y "${pkgs[@]}" || { log_fail "apt-get install failed"; return 1; }
            ;;
        dnf)
            local pkgs=(gcc pkgconfig dbus-devel openssl-devel curl at-spi2-core xdotool)
            log_info "sudo dnf install -y ${pkgs[*]}"
            sudo dnf install -y "${pkgs[@]}" || { log_fail "dnf install failed"; return 1; }
            ;;
        pacman)
            local pkgs=(base-devel pkgconf dbus openssl curl at-spi2-core xdotool)
            log_info "sudo pacman -S --needed --noconfirm ${pkgs[*]}"
            sudo pacman -S --needed --noconfirm "${pkgs[@]}" || { log_fail "pacman install failed"; return 1; }
            ;;
    esac
    log_ok "required system packages installed"
}

# -----------------------------------------------------------------------------
# Step 3: rustup toolchain
# -----------------------------------------------------------------------------

install_rust() {
    log_section "Step 3/6 — Rust toolchain"
    if [[ ${SKIP_RUST} -eq 1 ]]; then log_skip "--skip-rust"; return 0; fi

    if command -v cargo >/dev/null 2>&1; then
        log_ok "cargo already on PATH ($(cargo --version))"
        return 0
    fi
    if [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
        log_ok "cargo at ~/.cargo/bin (sourcing into PATH)"
        export PATH="${HOME}/.cargo/bin:${PATH}"
        return 0
    fi

    log_info "installing rustup (stable, minimal profile)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
        sh -s -- -y --default-toolchain stable --profile minimal --no-modify-path
    export PATH="${HOME}/.cargo/bin:${PATH}"
    command -v cargo >/dev/null 2>&1 || { log_fail "rustup install did not produce cargo"; return 1; }
    log_ok "rustup installed ($(cargo --version))"
}

# -----------------------------------------------------------------------------
# Step 4: cargo build + install binary into ~/.local/bin
# -----------------------------------------------------------------------------

build_and_install() {
    log_section "Step 4/6 — build & install binary"
    local built="${SCRIPT_DIR}/target/release/${BIN_NAME}"

    if [[ ${SKIP_BUILD} -eq 1 ]]; then
        log_skip "--skip-build (expecting a prebuilt binary at ${built})"
    else
        ( cd "${SCRIPT_DIR}" && cargo build --release ) || { log_fail "cargo build failed"; return 1; }
        log_ok "cargo build --release succeeded"
    fi

    [[ -x "${built}" ]] || { log_fail "binary not found at ${built}"; return 1; }

    mkdir -p "${INSTALL_DIR}"
    if [[ -f "${INSTALL_PATH}" ]] && cmp -s "${built}" "${INSTALL_PATH}"; then
        log_ok "binary already up to date at ${INSTALL_PATH}"
    else
        install -m 0755 "${built}" "${INSTALL_PATH}"
        log_ok "installed ${INSTALL_PATH}"
    fi

    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *) log_warn "${INSTALL_DIR} is not in your PATH — add it to your shell rc:"
           log_info "  export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
    esac
}

# -----------------------------------------------------------------------------
# Step 5: doctor readiness check# -----------------------------------------------------------------------------
# Step 5: doctor readiness check
# -----------------------------------------------------------------------------

doctor_install_prerequisites_ready_raw() {
    local out="$1" field
    for field in can_build_accessibility_tree can_send_input; do
        if ! printf '%s' "${out}" | grep -qE "\"${field}\"[[:space:]]*:[[:space:]]*true([[:space:],}]|$)"; then
            return 1
        fi
    done
}

run_doctor() {
    log_section "Step 5/6 — doctor readiness"
    if [[ ${SKIP_DOCTOR} -eq 1 ]]; then log_skip "--skip-doctor"; return 0; fi

    [[ -x "${INSTALL_PATH}" ]] || { log_fail "${INSTALL_PATH} missing — cannot run doctor"; return 1; }

    local out
    if ! out="$("${INSTALL_PATH}" doctor 2>&1)"; then
        log_fail "doctor invocation failed"
        printf '%s\n' "${out}"
        return 1
    fi

    if command -v jq >/dev/null 2>&1 && printf '%s' "${out}" | jq -e . >/dev/null 2>&1; then
        local blockers readiness_status
        blockers="$(printf '%s' "${out}" | jq -r '.readiness.blockers | if type == "array" then length else -1 end')"
        readiness_status="$(printf '%s' "${out}" | jq -r '
            .readiness as $r |
            if ($r | type) != "object"
                or ($r.blockers | type) != "array"
                or ($r.can_build_accessibility_tree | type) != "boolean"
                or ($r.can_send_input | type) != "boolean"
            then "invalid"
            elif $r.can_build_accessibility_tree != true
                or $r.can_send_input != true
            then "blocked"
            elif ($r.blockers | length) == 0 then "ready"
            else "degraded"
            end
        ')"
        printf '%s\n' "${out}" | jq -r '
            .readiness as $r |
            "fully ready: \($r.blockers | type == "array" and length == 0)\n" +
            ((($r.blockers // []) | map("  - \(.)") | join("\n")))
        '
        case "${readiness_status}" in
            ready)
                log_ok "doctor reports ready"
                ;;
            degraded)
                local capability_verb="capabilities are"
                if [[ "${blockers}" -eq 1 ]]; then capability_verb="capability is"; fi
                log_ok "installation prerequisites are ready"
                log_warn "installation succeeded; ${blockers} platform ${capability_verb} unavailable on this display"
                ;;
            blocked)
                log_fail "doctor reports NOT ready"
                while IFS= read -r line; do
                    FAILED_CHECKS+=("${line}")
                done < <(printf '%s' "${out}" | jq -r '.readiness.blockers[]?')
                return 1
                ;;
            *)
                log_fail "doctor output is missing required readiness fields — unexpected JSON structure"
                return 1
                ;;
        esac
    else
        # Raw fallback.
        printf '%s\n' "${out}"
        if printf '%s' "${out}" | grep -qiE '"blockers"[[:space:]]*:[[:space:]]*\[\]'; then
            log_ok "doctor reports ready (raw)"
        elif doctor_install_prerequisites_ready_raw "${out}"; then
            log_ok "installation prerequisites are ready (raw)"
            log_warn "installation succeeded; platform capabilities are unavailable on this display"
        else
            log_fail "doctor did not report ready (install jq for a structured summary)"
            return 1
        fi
    fi
}

# -----------------------------------------------------------------------------
# Driver
# -----------------------------------------------------------------------------

main() {
    printf '%scomputer-use-linux installer%s — repo: %s\n' "${C_BOLD}" "${C_RESET}" "${SCRIPT_DIR}"

    detect_distro
    install_system_deps  || record_failure "system deps"
    install_rust         || record_failure "rust toolchain"
    build_and_install    || record_failure "build/install"
    run_doctor           || record_failure "doctor"

    log_section "Step 6/6 — summary"
    if [[ ${#FAILED_CHECKS[@]} -eq 0 ]]; then
        log_ok "all steps completed successfully"
        exit 0
    else
        log_fail "completed with failures:"
        for f in "${FAILED_CHECKS[@]}"; do log_info "  - ${f}"; done
        exit 1
    fi
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi

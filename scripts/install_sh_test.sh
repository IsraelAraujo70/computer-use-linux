#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALLER="${ROOT_DIR}/install.sh"
FIXTURE_DIR="${ROOT_DIR}/tests/fixtures"

assert_eq() {
    local actual="$1" expected="$2"
    if [[ "${actual}" != "${expected}" ]]; then
        printf 'expected %q, got %q\n' "${expected}" "${actual}" >&2
        return 1
    fi
}

assert_contains() {
    local actual="$1" expected="$2"
    if [[ "${actual}" != *"${expected}"* ]]; then
        printf 'expected output to contain %q\n%s\n' "${expected}" "${actual}" >&2
        return 1
    fi
}

assert_not_contains() {
    local actual="$1" unexpected="$2"
    if [[ "${actual}" == *"${unexpected}"* ]]; then
        printf 'expected output not to contain %q\n%s\n' "${unexpected}" "${actual}" >&2
        return 1
    fi
}

create_fake_doctor() {
    local path="$1"
    printf '%s\n' \
        '#!/usr/bin/env bash' \
        'printf '\''%s\n'\'' "${COMPUTER_USE_LINUX_TEST_DOCTOR_OUTPUT}"' \
        >"${path}"
    chmod +x "${path}"
}

run_fake_doctor() {
    local doctor_output="$1" hide_jq="${2:-0}" fake_dir status
    fake_dir="$(mktemp -d)"
    INSTALL_PATH="${fake_dir}/computer-use-linux"
    create_fake_doctor "${INSTALL_PATH}"
    export COMPUTER_USE_LINUX_TEST_DOCTOR_OUTPUT="${doctor_output}"
    if [[ "${hide_jq}" -eq 1 ]]; then
        command() {
            if [[ "$1" == "-v" && "$2" == "jq" ]]; then return 1; fi
            builtin command "$@"
        }
    fi
    status=0
    run_doctor || status=$?
    rm -rf -- "${fake_dir}"
    return "${status}"
}

test_artix_selects_pacman() (
    export COMPUTER_USE_LINUX_OS_RELEASE_FILE="${FIXTURE_DIR}/os-release.artix"
    export XDG_SESSION_TYPE=x11
    export XDG_CURRENT_DESKTOP=unknown

    # shellcheck source=../install.sh
    source "${INSTALLER}"
    package_manager_available() { [[ "$1" == "pacman" ]]; }

    detect_distro >/dev/null || return 1
    assert_eq "${PKG_MANAGER}" "pacman" || return 1
    assert_eq "${DISTRO_FAMILY}" "arch" || return 1
)

test_unknown_distro_selects_only_available_manager() (
    export COMPUTER_USE_LINUX_OS_RELEASE_FILE="${FIXTURE_DIR}/os-release.unknown"
    export XDG_SESSION_TYPE=x11
    export XDG_CURRENT_DESKTOP=unknown

    # shellcheck source=../install.sh
    source "${INSTALLER}"
    FORCE_UNKNOWN_DISTRO=1
    package_manager_available() { [[ "$1" == "dnf" ]]; }

    detect_distro >/dev/null || return 1
    assert_eq "${PKG_MANAGER}" "dnf" || return 1
    assert_eq "${DISTRO_FAMILY}" "fedora" || return 1
)

test_skip_system_deps_needs_no_package_manager() (
    export COMPUTER_USE_LINUX_OS_RELEASE_FILE="${FIXTURE_DIR}/os-release.unknown"
    export XDG_SESSION_TYPE=x11
    export XDG_CURRENT_DESKTOP=unknown

    # shellcheck source=../install.sh
    source "${INSTALLER}"
    FORCE_UNKNOWN_DISTRO=1
    SKIP_SYSTEM_DEPS=1
    package_manager_available() { return 1; }

    detect_distro >/dev/null || return 1
    assert_eq "${PKG_MANAGER}" "" || return 1
    assert_eq "${DISTRO_FAMILY}" "unknown" || return 1
)

test_skip_system_deps_allows_missing_override() (
    export COMPUTER_USE_LINUX_OS_RELEASE_FILE="${FIXTURE_DIR}/os-release.unknown"
    export XDG_SESSION_TYPE=x11
    export XDG_CURRENT_DESKTOP=unknown

    # shellcheck source=../install.sh
    source "${INSTALLER}"
    PACKAGE_MANAGER_OVERRIDE=apt
    SKIP_SYSTEM_DEPS=1
    package_manager_available() { return 1; }

    detect_distro >/dev/null || return 1
    assert_eq "${PKG_MANAGER}" "apt" || return 1
    assert_eq "${DISTRO_FAMILY}" "debian" || return 1
)

test_system_deps_include_xdotool() (
    export COMPUTER_USE_LINUX_OS_RELEASE_FILE="${FIXTURE_DIR}/os-release.artix"
    export XDG_SESSION_TYPE=x11
    export DISPLAY=:0
    export XDG_CURRENT_DESKTOP=i3

    # shellcheck source=../install.sh
    source "${INSTALLER}"
    package_manager_available() { [[ "$1" == "pacman" ]]; }
    sudo() { printf 'sudo %s\n' "$*"; }

    local output
    detect_distro >/dev/null || return 1
    output="$(install_system_deps)" || return 1
    assert_contains "${output}" "pacman -S --needed --noconfirm" || return 1
    assert_contains "${output}" "xdotool" || return 1
    assert_contains "${output}" "at-spi2-core" || return 1
)

test_doctor_accepts_platform_capability_blockers() (
    # shellcheck source=../install.sh
    source "${INSTALLER}"
    local status output doctor_output
    doctor_output='{"readiness":{"can_build_accessibility_tree":true,"can_query_windows":false,"can_focus_windows":false,"can_send_input":true,"recommended_next_step":"Start i3.","blockers":["Window listing failed (i3-msg: could not connect). Start i3 on this DISPLAY and export I3SOCK for its IPC socket."]}}'

    status=0
    output="$(run_fake_doctor "${doctor_output}")" || status=$?

    assert_eq "${status}" "0" || return 1
    assert_contains "${output}" "installation succeeded; 1 platform capability is unavailable" || return 1
    assert_contains "${output}" "Window listing failed" || return 1
    assert_not_contains "${output}" "doctor reports NOT ready" || return 1
)

test_doctor_rejects_missing_install_prerequisite() (
    # shellcheck source=../install.sh
    source "${INSTALLER}"
    local status output doctor_output
    doctor_output='{"readiness":{"can_build_accessibility_tree":true,"can_query_windows":false,"can_focus_windows":false,"can_send_input":false,"recommended_next_step":"Install xdotool.","blockers":["Window listing failed (i3-msg: could not connect). Start i3 on this DISPLAY and export I3SOCK for its IPC socket.","xdotool is not installed; click, scroll, drag, type_text and press_key need it. Install xdotool."]}}'

    status=0
    output="$(run_fake_doctor "${doctor_output}")" || status=$?

    assert_eq "${status}" "1" || return 1
    assert_contains "${output}" "doctor reports NOT ready" || return 1
    assert_contains "${output}" "Install xdotool" || return 1
)

test_doctor_raw_fallback_accepts_platform_capability_blockers() (
    # shellcheck source=../install.sh
    source "${INSTALLER}"
    local status output doctor_output
    doctor_output='{"readiness":{"can_build_accessibility_tree":true,"can_query_windows":false,"can_focus_windows":false,"can_send_input":true,"recommended_next_step":"Start i3.","blockers":["Window listing failed (i3-msg: could not connect). Start i3 on this DISPLAY and export I3SOCK for its IPC socket."]}}'

    status=0
    output="$(run_fake_doctor "${doctor_output}" 1)" || status=$?

    assert_eq "${status}" "0" || return 1
    assert_contains "${output}" "installation succeeded; platform capabilities are unavailable" || return 1
    assert_not_contains "${output}" "doctor did not report ready" || return 1
)

test_doctor_raw_fallback_rejects_missing_install_prerequisite() (
    # shellcheck source=../install.sh
    source "${INSTALLER}"
    local status output doctor_output
    doctor_output='{"readiness":{"can_build_accessibility_tree":true,"can_query_windows":false,"can_focus_windows":false,"can_send_input":false,"recommended_next_step":"Install xdotool.","blockers":["xdotool is not installed; click, scroll, drag, type_text and press_key need it. Install xdotool."]}}'

    status=0
    output="$(run_fake_doctor "${doctor_output}" 1)" || status=$?

    assert_eq "${status}" "1" || return 1
    assert_contains "${output}" "doctor did not report ready" || return 1
)

run_test() {
    local name="$1" test_fn="$2"
    if "${test_fn}"; then
        printf 'ok - %s\n' "${name}"
    else
        printf 'not ok - %s\n' "${name}" >&2
        return 1
    fi
}

run_test "Artix selects pacman" test_artix_selects_pacman
run_test "unknown distro selects its only supported manager" test_unknown_distro_selects_only_available_manager
run_test "--skip-system-deps needs no package manager" test_skip_system_deps_needs_no_package_manager
run_test "--skip-system-deps allows a missing override" test_skip_system_deps_allows_missing_override
run_test "system dependencies include xdotool" test_system_deps_include_xdotool
run_test "doctor accepts platform capability blockers" test_doctor_accepts_platform_capability_blockers
run_test "doctor rejects missing install prerequisites" test_doctor_rejects_missing_install_prerequisite
run_test "doctor raw fallback accepts platform capability blockers" test_doctor_raw_fallback_accepts_platform_capability_blockers
run_test "doctor raw fallback rejects missing install prerequisites" test_doctor_raw_fallback_rejects_missing_install_prerequisite

#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || -z "${1}" ]]; then
  echo "Usage: $0 USER@HOST [ADB_SERIAL]" >&2
  exit 2
fi

remote_endpoint="${1}"
device_serial="${2:-${ANDROID_SERIAL:-}}"
package_id="com.example.wezterm_android"
task_data_root="${XDG_DATA_HOME:-${HOME}/.local/share}"
key_root="${task_data_root}/wezterm-android-debug"
key_file="${key_root}/wezterm_android_debug_ed25519"
device_key="files/ssh/identities/wezterm_android_debug_ed25519"

if [[ -z "${device_serial}" ]]; then
  device_serial="$(
    adb devices | awk 'NR > 1 && $2 == "device" { print $1; exit }'
  )"
fi
if [[ -z "${device_serial}" ]]; then
  echo "No authorized Android device was found." >&2
  exit 1
fi

install -d -m 700 "${key_root}"
if [[ ! -f "${key_file}" ]]; then
  ssh-keygen \
    -q \
    -t ed25519 \
    -N '' \
    -C 'wezterm-android-debug' \
    -f "${key_file}"
fi
chmod 600 "${key_file}"
chmod 644 "${key_file}.pub"

# This is idempotent. On the first run it may request the remote account's
# password once; subsequent runs authenticate with the dedicated key.
ssh-copy-id -i "${key_file}.pub" "${remote_endpoint}"

adb -s "${device_serial}" shell run-as "${package_id}" \
  mkdir -p files/ssh/identities
adb -s "${device_serial}" shell -T run-as "${package_id}" \
  dd "of=${device_key}" status=none \
  < "${key_file}"
adb -s "${device_serial}" shell run-as "${package_id}" \
  chmod 600 "${device_key}"

echo "Provisioned the app-private debug identity on ${device_serial}."
echo "The debug client will try it automatically on the next SSH connection."

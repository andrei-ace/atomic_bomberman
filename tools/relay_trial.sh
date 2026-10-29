#!/usr/bin/env bash
# Temporary real-WAN test listener; routes only this SSH client's new 443
# connections. The deployed relay stays running. systemd removes the mapping
# on stop, crash, or the fifteen-minute deadline.
set -euo pipefail
umask 077
trial_command=${1:?start, stop, status or invite required}
trial_root=${2:?private temporary build directory required}
[[ "$trial_root" == /var/tmp/atomic-authority-wan.* ]] || exit 2
trial_unit=atomic-bomberman-authority-trial
trial_table=atomic_bomberman_authority_trial
trial_binary="$trial_root/target/release/bomberman-relay"
trial_state="$trial_root/private"

case "$trial_command" in
  start)
    test -x "$trial_binary"
    systemctl is-active --quiet bomberman-relay
    if systemctl is-active --quiet "$trial_unit"; then echo 'A trial is already active' >&2; exit 2; fi
    if nft list table ip "$trial_table" >/dev/null 2>&1; then echo 'Trial table already exists' >&2; exit 2; fi
    trial_source=${SSH_CONNECTION%% *}
    python3 -c 'import ipaddress,sys;assert ipaddress.ip_address(sys.argv[1]).version==4' "$trial_source"
    systemctl show bomberman-relay -p MainPID --value > "$trial_root/production-pid"
    sha256sum /usr/local/lib/bomberman-relay/bomberman-relay > "$trial_root/production-sha256"
    trial_advertise=$(python3 - <<'PY'
import json
from urllib.parse import urlsplit
with open('/var/lib/bomberman-relay/private/config.json') as source:
    endpoint=urlsplit(json.load(source)['advertised_endpoint'])
assert endpoint.port in (None,443)
print(f'{endpoint.hostname}:443')
PY
)
    if [ ! -f "$trial_state/config.json" ]; then
      "$trial_binary" init --state "$trial_state" --listen 0.0.0.0:24443 --advertise "$trial_advertise" > /dev/null
    fi
    # Reuse the group's existing identity so saved links survive the trial.
    # Copy only on the server; credentials never pass through the checkout.
    chmod 700 "$trial_state"
    for trial_identity in certificate.pem certificate.der private-key.pem access-token; do
      install -m600 "/var/lib/bomberman-relay/private/$trial_identity" "$trial_state/$trial_identity"
      cmp -s "/var/lib/bomberman-relay/private/$trial_identity" "$trial_state/$trial_identity"
    done
    systemd-run --quiet --collect --unit "$trial_unit" \
      --property=RuntimeMaxSec=15min --property=MemoryMax=256M --property=TasksMax=64 \
      --property=NoNewPrivileges=yes \
      --property="ExecStopPost=/usr/sbin/nft delete table ip $trial_table" \
      "$trial_binary" serve --state "$trial_state"
    for trial_attempt in {1..50}; do
      if ss -Hlnut 'sport = :24443' | head -1 | read -r _; then break; fi
      sleep .1
    done
    systemctl is-active --quiet "$trial_unit"
    if ! nft -f - <<NFT
table ip $trial_table {
  chain input_mapping {
    type nat hook prerouting priority -110; policy accept;
    ip saddr $trial_source tcp dport 443 redirect to :24443
    ip saddr $trial_source udp dport 443 redirect to :24443
  }
}
NFT
    then systemctl stop "$trial_unit"; exit 1; fi
    echo 'Temporary trial active; only the test source is mapped; automatic cleanup in 15 minutes'
    ;;
  invite)
    "$trial_binary" export-invite --state "$trial_state"
    ;;
  stop)
    systemctl stop "$trial_unit" 2>/dev/null || true
    if nft list table ip "$trial_table" >/dev/null 2>&1; then nft delete table ip "$trial_table"; fi
    systemctl is-active --quiet bomberman-relay
    test "$(systemctl show bomberman-relay -p MainPID --value)" = "$(cat "$trial_root/production-pid")"
    sha256sum -c "$trial_root/production-sha256"
    echo 'Trial stopped; mapping removed; production process and binary unchanged'
    ;;
  status)
    systemctl show bomberman-relay "$trial_unit" -p Id -p ActiveState -p MainPID --no-pager
    ;;
  *) exit 2;;
esac

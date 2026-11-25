#!/bin/bash
# tools/hearth/e2e/all.sh IMAGE
#
# The whole end-to-end run on a fresh test bed: run.sh up, smoke.py,
# e2e.py, the ActiveSync replays against the running image
# (tools/activesync/replay.py), load.py, run.sh down. Each part prints
# PASS/FAIL lines; the exit status is 0 only when all of them passed.
# Needs a Python with dkimpy, pynacl, dnspython and websockets as $PYTHON
# (e2e.py and load.py); smoke.py and replay.py use the standard library.
# HM_E2E_KEEP=1 leaves the bed up afterwards (then run.sh down).
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
dir="${HM_PROOF_DIR:-/srv/hearth-mail-proof}"
image="${1:?image}"
py="${PYTHON:?PYTHON: a python with dkimpy, pynacl, dnspython, websockets}"
status=0
"$here/run.sh" up "$image" || exit 1
step() { echo "=== $1"; shift; "$@" || status=1; }
step smoke python3 -u "$root/tools/hearth/smoke.py"
step e2e "$py" -u "$here/e2e.py"
users() { python3 -I -c "import json,sys; u=json.load(open('$dir/e2e-users.json')); print(':'.join(u[sys.argv[1]][:2]))" "$1"; }
step activesync python3 -I -u "$root/tools/activesync/replay.py" https://127.0.0.1:28443 "$(users ann)" "$(users bob)"
step load "$py" -u "$here/load.py"
[ "${HM_E2E_KEEP:-}" = 1 ] || "$here/run.sh" down
echo "=== all: $([ $status = 0 ] && echo PASS || echo FAIL)"
exit $status

#!/bin/sh
# The whole test suite in the background (AGENTS.md, Development helpers): the report goes to
# build/test-reports/<name>.json and <name>.done appears when it finishes, so a session can wait
# on the marker and then read the summary:
#
#   tools/scripts/dev/suite.sh before-merge
#   until [ -f build/test-reports/before-merge.done ]; do sleep 15; done
#   python3 tools/scripts/dev/test_summary.py build/test-reports/before-merge.json
#
# It runs detached (nohup) so the caller's shell ending does not stop it. Do not edit engine
# sources while it runs: the scenario phase rebuilds the runtime from them.
set -e
cd "$(dirname "$0")/../../.."
name="${1:-latest}"
mkdir -p build/test-reports
rm -f "build/test-reports/$name.done"
export POCKET_ROOT="$PWD"
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Library/Developer/CommandLineTools}"
nohup sh -c "./.pocket/pocket test --json > 'build/test-reports/$name.json' 2>/dev/null; touch 'build/test-reports/$name.done'" > /dev/null 2>&1 &
echo "build/test-reports/$name.json"

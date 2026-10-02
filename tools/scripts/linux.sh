#!/bin/sh
# Build and test Pocket on Linux in a container (docs/build-system.md, Linux), from this working
# tree as it is (uncommitted changes included). The tree is copied into a Docker volume
# (pocket-linux) beside its own .pocket/deps and build/, so nothing here is touched.
#   tools/scripts/linux.sh            # build the tool, set up the dependencies, build debug, test
#   tools/scripts/linux.sh test --config release    # the same in release (debug's sanitizers want more than 20 GB with the CPU renderer)
#   tools/scripts/linux.sh build      # the same without the tests
#   tools/scripts/linux.sh shell      # a shell in the container, the copy at /work/aipocket
#   tools/scripts/linux.sh exec "<shell command>"   # run it in the copy, after building

#   tools/scripts/linux.sh <command>  # any pocket command, e.g. "run hello -- --headless --frames 60 --json"
#   POCKET_LINUX_ARCH=amd64 tools/scripts/linux.sh ...   # the same on x86_64 (its own image and volume)
# Needs Docker (colima start on a Mac; `colima start --vm-type vz --vz-rosetta` runs x86_64 through Rosetta).
set -e
root=$(cd "$(dirname "$0")/../.." && pwd)
arch=${POCKET_LINUX_ARCH:-}
image=pocket-linux${arch:+-$arch}
volume=$image
platform=${arch:+--platform linux/$arch}
if [ -n "$arch" ]; then
    # Another architecture's image needs BuildKit (`brew install docker-buildx`): the legacy
    # builder takes the local base image whatever the platform asked.
    docker buildx build -q $platform --load -t "$image" "$root/tools/docker/linux" >/dev/null
else
    docker build -q -t "$image" "$root/tools/docker/linux" >/dev/null
fi
step=${1:-test}
[ $# -gt 0 ] && shift
case "$step" in
    shell) cmd="bash" ;;
    build) cmd="./.pocket/pocket build $*" ;;
    exec) cmd="./.pocket/pocket build >/dev/null && $1" ;;
    test) cmd="./.pocket/pocket build $* && ./.pocket/pocket test $*" ;;
    *) cmd="./.pocket/pocket $step $*" ;;
esac
docker run --rm -i $platform $( [ "$step" = shell ] && echo -t ) -v "$root":/src:ro -v "$volume":/work "$image" bash -c "
    set -e
    mkdir -p /work/aipocket
    rsync -a --delete --exclude /build --exclude /.pocket/deps --exclude /.pocket/cache --exclude /.pocket/pocket --exclude /tools/pocket/target --exclude /dist --exclude /.codex --exclude /.git /src/ /work/aipocket/
    cd /work/aipocket
    export POCKET_ROOT=/work/aipocket CARGO_TARGET_DIR=/work/cargo-target
    cargo build -q --release --manifest-path tools/pocket/Cargo.toml
    mkdir -p .pocket && cp /work/cargo-target/release/pocket .pocket/pocket
    ./.pocket/pocket setup >/dev/null
    $cmd
"

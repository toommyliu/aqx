set -eu
artifacts=${MIRA_SKUA_ARTIFACTS:-"$PWD/skua/Host/artifacts"}
mkdir -p "$artifacts"
artifacts=$(cd "$artifacts" && pwd)
export MIRA_SKUA_DATA="${MIRA_SKUA_DATA:-$artifacts/data}"
export MIRA_SKUA_WORKER="$artifacts/publish/mira-skua"
dotnet publish skua/Host/Host.csproj -c Release --artifacts-path "$artifacts" -o "$artifacts/publish"
"$MIRA_SKUA_WORKER" fixtures skua/fixtures
test_file=${1:-'skua*'}
if [ "$#" -gt 0 ]; then shift; fi
cargo test -p mira-script --test "$test_file" -- --ignored "$@"

set -eu
artifacts=${AQX_SKUA_ARTIFACTS:-"$PWD/skua/Host/artifacts"}
mkdir -p "$artifacts"
artifacts=$(cd "$artifacts" && pwd)
export AQX_SKUA_DATA="${AQX_SKUA_DATA:-$artifacts/data}"
export AQX_SKUA_WORKER="$artifacts/publish/aqx-skua"
dotnet publish skua/Host/Host.csproj -c Release --artifacts-path "$artifacts" -o "$artifacts/publish"
"$AQX_SKUA_WORKER" fixtures skua/fixtures
test_file=${1:-'skua*'}
if [ "$#" -gt 0 ]; then shift; fi
cargo test -p aqx-script --test "$test_file" -- --ignored "$@"

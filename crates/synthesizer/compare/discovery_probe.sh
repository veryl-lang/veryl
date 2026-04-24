#!/usr/bin/env bash
# Probe each support/discovery/build/*/*/Veryl.toml project for:
#   (a) veryl build succeeds
#   (b) which top modules synth cleanly (i.e. report a nonzero area)
#
# Prints one row per module: <status> <project> <top>
#
# Status legend:
#   BUILD_FAIL      veryl build itself failed
#   SYNTH_OK        veryl synth returned a nonzero area (record candidate)
#   SYNTH_ZERO      veryl synth returned 0 area (submodule-only wrapper)
#   SYNTH_FAIL      veryl synth emitted an error

set -eu

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
DISCOVERY="$REPO_ROOT/support/discovery/build"

if [[ -z "${VERYL:-}" ]]; then
  for cand in "$REPO_ROOT/target/release-verylup/veryl" \
              "$REPO_ROOT/target/release/veryl" \
              "$REPO_ROOT/target/debug/veryl"; do
    [[ -x "$cand" ]] && { export VERYL="$cand"; break; }
  done
fi

# Minimal per-project probe. We build once, then try every `module <name>`
# declaration found under src/ as a synth top. 0-area results are reported
# but skipped as benchmarks (they're usually instantiation wrappers).
probe_project() {
  local proj_dir="$1"
  local proj_name="$(basename "$(dirname "$proj_dir")")/$(basename "$proj_dir")"

  if ! ( cd "$proj_dir" && timeout 30 "$VERYL" build >/dev/null 2>&1 ); then
    echo "BUILD_FAIL  $proj_name  -"
    return
  fi

  local tops
  # Pick out every `module foo` / `pub module foo` declaration. Skip the
  # explicit `test_*` helpers — they often lack drivers for ports.
  tops=$(grep -r --include='*.veryl' -hE '^[[:space:]]*(pub[[:space:]]+)?module[[:space:]]+[A-Za-z_][A-Za-z_0-9]*' "$proj_dir/src" 2>/dev/null \
    | sed -nE 's/^[[:space:]]*(pub[[:space:]]+)?module[[:space:]]+([A-Za-z_][A-Za-z_0-9]*).*/\2/p' \
    | grep -v '^test_' | sort -u)

  if [[ -z "$tops" ]]; then
    echo "NO_MODULE   $proj_name  -"
    return
  fi

  for top in $tops; do
    local log area
    # 30 s per synth keeps one misbehaving module from blocking the whole sweep.
    log=$(cd "$proj_dir" && timeout 30 "$VERYL" synth --quiet --top "$top" 2>&1 || true)
    area=$(echo "$log" | sed -nE 's/^[[:space:]]*area:[[:space:]]+([0-9.]+).*/\1/p' | head -1)
    if [[ -z "$area" ]]; then
      echo "SYNTH_FAIL  $proj_name  $top"
    elif [[ "$area" == "0.00" ]]; then
      echo "SYNTH_ZERO  $proj_name  $top"
    else
      echo "SYNTH_OK    $proj_name  $top  area=$area"
    fi
  done
}

while IFS= read -r toml; do
  probe_project "$(dirname "$toml")"
done < <(find "$DISCOVERY" -maxdepth 3 -name Veryl.toml)

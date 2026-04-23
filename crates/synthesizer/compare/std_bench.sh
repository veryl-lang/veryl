#!/usr/bin/env bash
# Driver that runs compare.sh across the synthesizable subset of crates/std.
#
# Usage (from repo root):
#   crates/synthesizer/compare/std_bench.sh [-o <csv>]
#
# Requires `veryl build` to have been run in crates/std/veryl beforehand so
# that the .sv files under target/src/ exist.
#
# The module list below is maintained by hand. Modules that hit unsupported
# constructs at default parameters (dynamic index, etc.) are omitted rather
# than reported as NA — their presence would dilute the calibration fit.

set -eu

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
PROJECT="$REPO_ROOT/crates/std/veryl"

# yosys ships as a wrapper that resolves its lib/ dir relative to its own
# bin/. Symlinking it would break lib lookup, so default YOSYS to the
# real path when oss-cad-suite is installed at /tmp/oss-cad-suite-install.
if [[ -z "${YOSYS:-}" && -x /tmp/oss-cad-suite-install/oss-cad-suite/bin/yosys ]]; then
  export YOSYS=/tmp/oss-cad-suite-install/oss-cad-suite/bin/yosys
fi
# Prefer the in-tree build over any older `veryl` on PATH.
if [[ -z "${VERYL:-}" ]]; then
  for cand in "$REPO_ROOT/target/release-verylup/veryl" \
              "$REPO_ROOT/target/release/veryl" \
              "$REPO_ROOT/target/debug/veryl"; do
    [[ -x "$cand" ]] && { export VERYL="$cand"; break; }
  done
fi

csv_out=""
while getopts "o:h" opt; do
  case "$opt" in
    o) csv_out="$OPTARG" ;;
    h) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option" >&2; exit 1 ;;
  esac
done

# Fresh CSV: wipe before the first module so compare.sh re-writes the header.
if [[ -n "$csv_out" ]]; then
  : > "$csv_out"
fi

# Each entry: veryl_top | sv_top | src_stem | extra_pkg_stems_csv
# src_stem is the path under PROJECT without extension; compare.sh resolves
# it to `target/src/<stem>.sv` (emitted) and `<stem>.veryl` (source).
# extra_pkg_stems lists any _pkg / submodule stems the top depends on.
entries=(
  'counter|std_counter|src/counter/counter|'
  'gray_encoder|std_gray_encoder|src/gray/gray_encoder|'
  'ram|std_ram|src/ram/ram|'
  'delay|std_delay|src/delay/delay|'
  'edge_detector|std_edge_detector|src/edge_detector/edge_detector|'
  'mux|std_mux|src/selector/mux|src/selector/selector_pkg'
  'demux|std_demux|src/selector/demux|src/selector/selector_pkg'
)
# Omitted: gray_counter / gray_decoder (veryl synth returns 0 for modules
# that just instantiate a submodule), std_fifo (yosys hierarchy issue with
# fifo_controller submodule), std_async_handshake (sv2v fail).

# Build `stem → sv_path:veryl_path` strings in the `src.sv:src.veryl` form
# that compare.sh understands.
resolve_pair() {
  local stem="$1"
  echo "target/src/${stem}.sv:${stem}.veryl"
}

for entry in "${entries[@]}"; do
  IFS='|' read -r veryl_top sv_top stem extra_stems <<<"$entry"
  mod_arg="${veryl_top}:${sv_top}:$(resolve_pair "$stem")"
  args=(-C "$PROJECT" -m "$mod_arg")
  if [[ -n "$extra_stems" ]]; then
    pkg_list=""
    IFS=',' read -r -a stems <<<"$extra_stems"
    for s in "${stems[@]}"; do
      pair=$(resolve_pair "$s")
      pkg_list+="${pkg_list:+,}$pair"
    done
    args+=(-p "$pkg_list")
  fi
  [[ -n "$csv_out" ]] && args+=(-o "$csv_out")
  "$SCRIPT_DIR/compare.sh" "${args[@]}"
done

#!/usr/bin/env bash
# Wrapper around compare.sh that drives the heliodor submodule benchmark set.
#
# Usage (from repo root):
#   crates/synthesizer/compare/heliodor_bench.sh [-o <csv>]
#
# Assumes testcases/heliodor has already been built (`veryl build` run there)
# so that the .sv files referenced in -m exist.

set -eu

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
PROJECT="$REPO_ROOT/testcases/heliodor"

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

pkgs='src/pkg/riscv_pkg.sv,src/pkg/alu_pkg.sv,src/pkg/pipeline_pkg.sv'
pkgs+=',src/pkg/csr_pkg.sv,src/pkg/fpu_pkg.sv'

# Small combinational / control-oriented submodules. ALU and FP blocks are
# skipped — they take yosys minutes-to-hours and aren't needed for the
# calibration fit target (the point is diversity, not full coverage).
mods=(
  'imm_gen:heliodor_imm_gen:src/core/imm_gen.sv'
  'decoder:heliodor_decoder:src/core/decoder.sv'
  'branch_comp:heliodor_branch_comp:src/core/branch_comp.sv'
  'forwarding_unit:heliodor_forwarding_unit:src/core/forwarding_unit.sv'
  'hazard_unit:heliodor_hazard_unit:src/core/hazard_unit.sv'
  'c_expander:heliodor_c_expander:src/core/c_expander.sv'
)

mod_csv=$(IFS=','; echo "${mods[*]}")

args=(-C "$PROJECT" -p "$pkgs" -m "$mod_csv")
[[ -n "$csv_out" ]] && args+=(-o "$csv_out")

exec "$SCRIPT_DIR/compare.sh" "${args[@]}"

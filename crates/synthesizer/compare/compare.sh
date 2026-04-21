#!/usr/bin/env bash
# Compare `veryl synth` output against Yosys + sky130 technology mapping.
#
# Required tools (place paths in env vars; defaults assume they live next to
# this script under ./tools/):
#   YOSYS    : yosys binary (e.g., from oss-cad-suite)
#   SV2V     : sv2v binary (SystemVerilog-2017 -> Verilog-2005)
#   LIBERTY  : sky130_fd_sc_hd Liberty file (typical corner)
#   VERYL    : veryl binary (defaults to `veryl` on PATH)
#
# The target project must already have been built with `veryl build` so that
# both Veryl sources (for `veryl synth`) and emitted .sv files (for yosys)
# are present.
#
# Usage:
#   compare.sh -C <project-dir> \
#              -p <pkg.sv,pkg.sv,...> \
#              -m <veryl_top:sv_top:src.sv,...>
#
# veryl_top : module name as it appears in the .veryl source (bare identifier)
# sv_top    : module name after SV emission (typically ${project}_${name})
# src.sv    : emitted .sv file (relative to project-dir)
#
# Example (heliodor, cwd = repo root):
#   compare.sh -C testcases/heliodor \
#     -p src/pkg/riscv_pkg.sv,src/pkg/alu_pkg.sv,src/pkg/pipeline_pkg.sv,\
# src/pkg/csr_pkg.sv,src/pkg/fpu_pkg.sv \
#     -m imm_gen:heliodor_imm_gen:src/core/imm_gen.sv,\
# decoder:heliodor_decoder:src/core/decoder.sv

set -eu
# `head -1` in some pipelines below intentionally closes early, producing
# SIGPIPE on the upstream sed/grep. With pipefail on, the whole script
# aborts — so we leave pipefail off and rely on explicit NA fallbacks.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TOOLS_DIR="${TOOLS_DIR:-$SCRIPT_DIR/tools}"

YOSYS="${YOSYS:-$TOOLS_DIR/yosys}"
SV2V="${SV2V:-$TOOLS_DIR/sv2v}"
LIBERTY="${LIBERTY:-$TOOLS_DIR/sky130_fd_sc_hd.lib}"
VERYL="${VERYL:-veryl}"

project_dir=""
pkg_csv=""
mod_csv=""
timing_paths=0
while getopts "C:p:m:t:h" opt; do
  case "$opt" in
    C) project_dir="$OPTARG" ;;
    p) pkg_csv="$OPTARG" ;;
    m) mod_csv="$OPTARG" ;;
    t) timing_paths="$OPTARG" ;;
    h) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option" >&2; exit 1 ;;
  esac
done

if [[ -z "$project_dir" || -z "$mod_csv" ]]; then
  echo "error: -C <project-dir> and -m <modules> are required" >&2
  exit 1
fi

for tool in "$YOSYS" "$SV2V" "$LIBERTY"; do
  if [[ ! -e "$tool" ]]; then
    echo "error: missing $tool (override via env: YOSYS=, SV2V=, LIBERTY=)" >&2
    exit 1
  fi
done
if ! command -v "$VERYL" >/dev/null 2>&1 && [[ ! -x "$VERYL" ]]; then
  echo "error: veryl binary not found (override via env VERYL=)" >&2
  exit 1
fi

project_abs="$(cd "$project_dir" && pwd)"
WORK="${WORK:-$(mktemp -d)}"
mkdir -p "$WORK"

abc_script="$WORK/abc_script.txt"
cat > "$abc_script" <<'ABCEOF'
strash
map -p
topo
stime -p
ABCEOF

IFS=',' read -r -a pkgs <<<"$pkg_csv"
IFS=',' read -r -a mods <<<"$mod_csv"

pkg_abs=()
for p in "${pkgs[@]}"; do
  [[ -z "$p" ]] && continue
  pkg_abs+=("$project_abs/$p")
done

printf '%-28s %12s %12s %8s %12s %12s %8s\n' \
  'module' 'veryl_area' 'yosys_area' 'a_ratio' 'veryl_delay' 'yosys_delay' 'd_ratio'
printf '%-28s %12s %12s %8s %12s %12s %8s\n' \
  '------' '----------' '----------' '-------' '-----------' '-----------' '-------'

for entry in "${mods[@]}"; do
  veryl_top="${entry%%:*}"
  rest="${entry#*:}"
  sv_top="${rest%%:*}"
  src_rel="${rest#*:}"
  src_abs="$project_abs/$src_rel"

  # --- Our synthesizer -----------------------------------------------------
  # Pass explicit .veryl files for the pkgs and the top source. This skips
  # the full-project scan (which otherwise dominates wall time for small
  # modules by parsing ~100 .veryl files of the project). We derive each
  # .veryl path by swapping the .sv extension — assumes a source-target
  # project layout where emitted .sv sits next to its .veryl origin.
  veryl_pkgs=()
  for p in "${pkg_abs[@]}"; do
    veryl_pkgs+=("${p%.sv}.veryl")
  done
  veryl_src="${src_abs%.sv}.veryl"
  veryl_log=$(cd "$project_abs" && "$VERYL" synth --quiet --top "$veryl_top" \
    "${veryl_pkgs[@]}" "$veryl_src" 2>&1 || true)
  # New format: "area: 752.50 um²  (comb ...)" / "timing: 0.580 ns  6 gates  ..."
  veryl_area=$(echo "$veryl_log" \
    | sed -nE 's/^area:[[:space:]]+([0-9.]+).*/\1/p' | head -1)
  veryl_delay=$(echo "$veryl_log" \
    | sed -nE 's/^timing:[[:space:]]+([0-9.]+)[[:space:]]*ns.*/\1/p' | head -1)
  veryl_area="${veryl_area:-NA}"
  veryl_delay="${veryl_delay:-NA}"

  # --- Yosys + sky130 ---
  v2005="$WORK/${sv_top}.v"
  if ! "$SV2V" --top "$sv_top" "${pkg_abs[@]}" "$src_abs" -w "$v2005" 2>"$WORK/${sv_top}.sv2v.err"; then
    printf '%-28s %12s %12s %8s %12s %12s %8s\n' \
      "$sv_top" "$veryl_area" 'sv2v_fail' '-' "$veryl_delay" '-' '-'
    continue
  fi

  ylog=$("$YOSYS" -p "
    read_verilog $v2005
    hierarchy -top $sv_top
    synth -top $sv_top
    dfflibmap -liberty $LIBERTY
    abc -liberty $LIBERTY -script $abc_script
    stat -liberty $LIBERTY
  " 2>&1 || true)

  y_area=$(echo "$ylog" | awk '/Chip area/ {print $NF; exit}')
  y_delay=$(echo "$ylog" | grep 'ABC: WireLoad' | head -1 \
    | sed -E 's/.*Delay *= *([0-9.]+) *ps.*/\1/')
  y_area="${y_area:-NA}"
  y_delay="${y_delay:-NA}"

  a_ratio='-'; d_ratio='-'
  if [[ "$veryl_area" != "NA" && "$y_area" != "NA" ]]; then
    a_ratio=$(awk -v a="$veryl_area" -v b="$y_area" 'BEGIN{printf "%.2f", a/b}')
  fi
  if [[ "$veryl_delay" != "NA" && "$y_delay" != "NA" ]]; then
    # veryl delay is ns, yosys delay is ps -> convert ns to ps
    d_ratio=$(awk -v a="$veryl_delay" -v b="$y_delay" \
      'BEGIN{printf "%.2f", (a*1000)/b}')
  fi

  printf '%-28s %12s %12s %8s %12s %12s %8s\n' \
    "$sv_top" "$veryl_area" "$y_area" "$a_ratio" "$veryl_delay" "$y_delay" "$d_ratio"

  # --- Timing detail (-t N): show our top-N paths and yosys's single path
  # side-by-side so the endpoints and path structure can be compared.
  if [[ "$timing_paths" -gt 0 ]]; then
    echo "    yosys path:"
    echo "$ylog" | awk '/ABC: Start-point/ {print "      "$0}' || true
    echo "$ylog" \
      | awk '/ABC: Path [0-9]+ --/ {
          delay = ""
          for (i=1; i<=NF; i++) if ($i == "Df") { delay = $(i+2); break }
          cell = ""
          for (i=1; i<=NF; i++) if ($i ~ /^sky130_/) { cell = $i; break }
          printf "      Df=%s ps cell=%s\n", delay, cell
        }' || true
    echo "    veryl top-$timing_paths:"
    timing_out=$(cd "$project_abs" && "$VERYL" synth --quiet --top "$veryl_top" \
        "${veryl_pkgs[@]}" "$veryl_src" --dump-timing --timing-paths "$timing_paths" 2>&1 || true)
    echo "$timing_out" \
      | awk '/^-- rank/ {rank=$0; next} /^  from/ {print "      "rank"  "$0}' || true
  fi
done

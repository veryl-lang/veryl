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
#              -m <veryl_top:sv_top:src.sv,...> \
#              [-o <csv-path>]
#
# veryl_top : module name as it appears in the .veryl source (bare identifier)
# sv_top    : module name after SV emission (typically ${project}_${name})
# src.sv    : emitted .sv file (relative to project-dir)
# -o <csv>  : also emit one CSV row per module with a fuller feature set
#             (cell-kind breakdown, FF count) for offline analysis. The file
#             is created fresh on first invocation and appended thereafter —
#             remove it manually before re-running a benchmark set.
#
# A 4th optional field may be appended to each -m entry when the .veryl
# source does not sit next to its .sv (e.g. when the project builds into
# `target/src/`):
#   veryl_top:sv_top:src.sv:veryl.veryl
# Same applies to the -p list: each pkg may be `pkg.sv` or `pkg.sv:pkg.veryl`.
# When omitted, the .veryl path is derived by swapping the .sv extension.
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
csv_out=""
while getopts "C:p:m:t:o:h" opt; do
  case "$opt" in
    C) project_dir="$OPTARG" ;;
    p) pkg_csv="$OPTARG" ;;
    m) mod_csv="$OPTARG" ;;
    t) timing_paths="$OPTARG" ;;
    o) csv_out="$OPTARG" ;;
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

# Pkg entries may carry an explicit .veryl override (pkg.sv:pkg.veryl) when
# the .veryl tree and the emitted .sv tree diverge. Stash both absolute paths.
pkg_sv_abs=()
pkg_veryl_abs=()
for p in "${pkgs[@]}"; do
  [[ -z "$p" ]] && continue
  case "$p" in
    *:*)
      sv_rel="${p%%:*}"
      vy_rel="${p#*:}"
      pkg_sv_abs+=("$project_abs/$sv_rel")
      pkg_veryl_abs+=("$project_abs/$vy_rel")
      ;;
    *)
      pkg_sv_abs+=("$project_abs/$p")
      pkg_veryl_abs+=("$project_abs/${p%.sv}.veryl")
      ;;
  esac
done

printf '%-28s %12s %12s %8s %12s %12s %8s\n' \
  'module' 'veryl_area' 'yosys_area' 'a_ratio' 'veryl_delay' 'yosys_delay' 'd_ratio'
printf '%-28s %12s %12s %8s %12s %12s %8s\n' \
  '------' '----------' '----------' '-------' '-----------' '-----------' '-------'

if [[ -n "$csv_out" && ( ! -s "$csv_out" ) ]]; then
  # Only write the header when the file is new/empty. Re-invocations that
  # target the same CSV simply append — useful for std_bench.sh which runs
  # compare.sh once per module.
  echo 'module,veryl_area,yosys_area,a_ratio,veryl_delay,yosys_delay,d_ratio,veryl_time,yosys_time,ff_count,cells' \
    > "$csv_out"
fi

for entry in "${mods[@]}"; do
  veryl_top="${entry%%:*}"
  rest="${entry#*:}"
  sv_top="${rest%%:*}"
  rest2="${rest#*:}"
  # Allow an optional 4th field specifying the .veryl source explicitly —
  # needed when the emitted .sv lives under `target/src/` rather than next
  # to the .veryl.
  if [[ "$rest2" == *:* ]]; then
    src_rel="${rest2%%:*}"
    veryl_rel="${rest2#*:}"
  else
    src_rel="$rest2"
    veryl_rel="${src_rel%.sv}.veryl"
  fi
  src_abs="$project_abs/$src_rel"
  veryl_src="$project_abs/$veryl_rel"

  # --- Our synthesizer -----------------------------------------------------
  # Pass explicit .veryl files for the pkgs and the top source. This skips
  # the full-project scan (which otherwise dominates wall time for small
  # modules by parsing ~100 .veryl files of the project).
  veryl_pkgs=("${pkg_veryl_abs[@]}")
  # --dump-area is added so the CSV path can record per-cell-kind counts.
  # It is cheap — the breakdown is a side output of an already-computed report.
  # Time the invocation so the CSV can carry per-module wall-clock figures.
  veryl_t_start=$(date +%s.%N)
  veryl_log=$(cd "$project_abs" && timeout 120 "$VERYL" synth --quiet --top "$veryl_top" \
    --dump-area \
    "${veryl_pkgs[@]}" "$veryl_src" 2>&1 || true)
  veryl_t_end=$(date +%s.%N)
  veryl_time=$(awk -v s="$veryl_t_start" -v e="$veryl_t_end" 'BEGIN{printf "%.3f", e-s}')
  # Summary lines are indented under a `summary:` header, e.g.
  #   "  area:        267.50 um²  (comb ...)"
  #   "  timing:       1.100 ns       7 levels  ..."
  # so allow optional leading whitespace before the label.
  veryl_area=$(echo "$veryl_log" \
    | sed -nE 's/^[[:space:]]*area:[[:space:]]+([0-9.]+).*/\1/p' | head -1)
  veryl_delay=$(echo "$veryl_log" \
    | sed -nE 's/^[[:space:]]*timing:[[:space:]]+([0-9.]+)[[:space:]]*ns.*/\1/p' | head -1)
  veryl_area="${veryl_area:-NA}"
  veryl_delay="${veryl_delay:-NA}"

  # Cell-kind breakdown: lines like `  and2   × 5  25.00` under "area:".
  # The × and the count may or may not be whitespace-separated (`× 5` vs
  # `×12`), so strip everything through the × plus trailing spaces and
  # coerce the remainder to a number — awk takes the leading integer.
  # FF is extracted separately since it goes into ff_count for convenience.
  veryl_cells=$(echo "$veryl_log" | awk '
    /^area:$/ {in_area=1; next}
    /^timing:/ {in_area=0}
    in_area && /^  [A-Za-z]/ {
      name=$1
      rest=$0
      sub(/^[^×]*×[[:space:]]*/, "", rest)
      count = rest + 0
      if (name == "FF") next
      printf "%s%s=%d", sep, name, count; sep=";"
    }')
  veryl_ff=$(echo "$veryl_log" | awk '
    /^area:$/ {in_area=1; next}
    /^timing:/ {in_area=0}
    in_area && /^  FF/ {
      rest=$0
      sub(/^[^×]*×[[:space:]]*/, "", rest)
      print (rest + 0); exit
    }')
  veryl_cells="${veryl_cells:-NA}"
  veryl_ff="${veryl_ff:-0}"

  # --- Yosys + sky130 ---
  v2005="$WORK/${sv_top}.v"
  if ! "$SV2V" --top "$sv_top" "${pkg_sv_abs[@]}" "$src_abs" -w "$v2005" 2>"$WORK/${sv_top}.sv2v.err"; then
    printf '%-28s %12s %12s %8s %12s %12s %8s\n' \
      "$sv_top" "$veryl_area" 'sv2v_fail' '-' "$veryl_delay" '-' '-'
    continue
  fi

  # sv2v may rename parameterised top modules (e.g. `jellyvl_stream_ff` →
  # `jellyvl_stream_ff_8BAAD`) when another site instantiates them with
  # concrete generics. When that happens the `--top` filter produces an
  # empty file — retry without `--top`, then scan the emitted SV for a
  # module name that begins with the original `sv_top` to feed yosys.
  yosys_top="$sv_top"
  if [[ ! -s "$v2005" ]]; then
    "$SV2V" "${pkg_sv_abs[@]}" "$src_abs" -w "$v2005" 2>"$WORK/${sv_top}.sv2v.err" || true
    yosys_top=$(grep -oE "^module[[:space:]]+${sv_top}[A-Za-z0-9_]*" "$v2005" \
                | head -1 | awk '{print $2}')
    yosys_top="${yosys_top:-$sv_top}"
  fi

  yosys_t_start=$(date +%s.%N)
  ylog=$(timeout 600 "$YOSYS" -p "
    read_verilog $v2005
    hierarchy -top $yosys_top
    synth -top $yosys_top
    dfflibmap -liberty $LIBERTY
    abc -liberty $LIBERTY -script $abc_script
    stat -liberty $LIBERTY
  " 2>&1 || true)
  yosys_t_end=$(date +%s.%N)
  yosys_time=$(awk -v s="$yosys_t_start" -v e="$yosys_t_end" 'BEGIN{printf "%.3f", e-s}')

  # yosys `stat` emits one "Chip area" per module when the design has
  # sub-hierarchy, then a final "Chip area for top module" with the
  # flattened total. Prefer that line; fall back to any "Chip area"
  # line when only one is present (flat designs).
  y_area=$(echo "$ylog" | awk '/Chip area for top module/ {print $NF}' | tail -1)
  if [[ -z "$y_area" ]]; then
    y_area=$(echo "$ylog" | awk '/Chip area/ {print $NF}' | tail -1)
  fi
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

  if [[ -n "$csv_out" ]]; then
    printf '%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,"%s"\n' \
      "$sv_top" "$veryl_area" "$y_area" "$a_ratio" \
      "$veryl_delay" "$y_delay" "$d_ratio" \
      "$veryl_time" "$yosys_time" \
      "$veryl_ff" "$veryl_cells" \
      >> "$csv_out"
  fi

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

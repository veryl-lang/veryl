#!/usr/bin/env bash
# Drive compare.sh across every (project, top) pair that discovery_probe.sh
# tagged as SYNTH_OK. Usage (from repo root):
#   crates/synthesizer/compare/discovery_bench.sh [-o <csv>] [-p <probe.log>]
#
# Modules that require cross-file hierarchy usually yield yosys-NA — we keep
# those rows in the CSV for completeness (they're easy to filter out when
# aggregating).

set -eu

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
DISCOVERY="$REPO_ROOT/support/discovery/build"

if [[ -z "${YOSYS:-}" && -x /tmp/oss-cad-suite-install/oss-cad-suite/bin/yosys ]]; then
  export YOSYS=/tmp/oss-cad-suite-install/oss-cad-suite/bin/yosys
fi
if [[ -z "${VERYL:-}" ]]; then
  for cand in "$REPO_ROOT/target/release-verylup/veryl" \
              "$REPO_ROOT/target/release/veryl" \
              "$REPO_ROOT/target/debug/veryl"; do
    [[ -x "$cand" ]] && { export VERYL="$cand"; break; }
  done
fi

csv_out=""
probe_log="$SCRIPT_DIR/discovery_probe.log"
while getopts "o:p:h" opt; do
  case "$opt" in
    o) csv_out="$OPTARG" ;;
    p) probe_log="$OPTARG" ;;
    h) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option" >&2; exit 1 ;;
  esac
done

if [[ ! -s "$probe_log" ]]; then
  echo "error: probe log $probe_log is empty; run discovery_probe.sh first" >&2
  exit 1
fi

# Caller owns the CSV lifecycle — compare.sh appends when the file exists,
# so wipe here so the first invocation writes a fresh header.
[[ -n "$csv_out" ]] && : > "$csv_out"

# For each SYNTH_OK row, try to resolve the .sv and .veryl paths.
# Strategy:
#   1. .veryl:  find <top>.veryl anywhere under src/
#   2. .sv:     locate the file that contains `module <anything_ending_in_top>`
#               — discovers the emitted sv_top even when the project prefixes
#               the name (e.g. vips_Alu, std_counter).
# Modules whose paths can't be resolved are skipped.
while IFS= read -r line; do
  [[ "$line" =~ ^SYNTH_OK ]] || continue
  proj=$(awk '{print $2}' <<<"$line")
  top=$(awk '{print $3}' <<<"$line")
  proj_dir="$DISCOVERY/$proj"
  [[ -d "$proj_dir" ]] || continue

  # Locate .veryl by grep (filename may have a project prefix, e.g.
  # `jellyvl_cdc_array_single.veryl`). Anchor the module keyword and exclude
  # bundled dependencies so we pick the in-tree source.
  veryl_path=$(grep -lrE "^[[:space:]]*(pub[[:space:]]+)?module[[:space:]]+${top}[^A-Za-z0-9_]" \
               --include='*.veryl' "$proj_dir/src" 2>/dev/null \
               | grep -v '/dependencies/' | head -1)
  [[ -z "$veryl_path" ]] && continue
  veryl_rel="${veryl_path#$proj_dir/}"

  # Search emitted .sv for a matching module declaration. The top identifier
  # may be prefixed in the .sv (e.g. `vips_Alu`, `std_counter`), so we allow
  # any prefix that ends with the bare `<top>` identifier.
  sv_info=$(grep -lrE "^module[[:space:]]+([A-Za-z0-9_]+_)?${top}[^A-Za-z0-9_]" \
            --include='*.sv' "$proj_dir" 2>/dev/null \
            | grep -v '/dependencies/' \
            | grep -v '/tests/' | grep -v '/tb/' \
            | head -1)
  [[ -z "$sv_info" ]] && continue
  sv_rel="${sv_info#$proj_dir/}"
  sv_top=$(grep -oE "^module[[:space:]]+([A-Za-z0-9_]+_)?${top}([^A-Za-z0-9_]|$)" \
           "$sv_info" | head -1 | awk '{print $2}' | tr -d '[:space:]#(')

  # Collect every OTHER .sv in the project tree (plus its matching .veryl)
  # so sub-module dependencies are supplied to sv2v + yosys and to veryl
  # synth. Without this, hierarchical designs fail on yosys with
  # "Module X is not part of the design" and veryl silently blackboxes
  # unresolved instances, producing an incomplete area/timing read.
  deps_list=""
  while IFS= read -r dep_sv_abs; do
    dep_sv_rel="${dep_sv_abs#$proj_dir/}"
    [[ "$dep_sv_rel" == "$sv_rel" ]] && continue
    # Skip files that contain non-synthesizable constructs — yosys's
    # Verilog frontend errors out on `real`, `initial`, `$display`, etc.
    # even if the module is not selected by `hierarchy -top`.
    if grep -qE "^[[:space:]]+real[[:space:]]" "$dep_sv_abs" 2>/dev/null; then
      continue
    fi
    # Match the corresponding .veryl by module identifier extracted from
    # the .sv (the emitted SV prefixes modules like `jellyvl_cdc_gray`;
    # the bare Veryl module name is the suffix after the final `_`).
    dep_mod=$(grep -oE "^module[[:space:]]+[A-Za-z0-9_]+" "$dep_sv_abs" 2>/dev/null \
              | head -1 | awk '{print $2}')
    [[ -z "$dep_mod" ]] && { deps_list+="${dep_sv_rel},"; continue; }
    dep_stem="${dep_mod#*_}"
    dep_veryl=$(grep -lrE "^[[:space:]]*(pub[[:space:]]+)?(module|package|interface|proto)[[:space:]]+(${dep_mod}|${dep_stem})[^A-Za-z0-9_]" \
                --include='*.veryl' "$proj_dir/src" 2>/dev/null \
                | grep -v '/dependencies/' | head -1)
    if [[ -n "$dep_veryl" ]]; then
      deps_list+="${dep_sv_rel}:${dep_veryl#$proj_dir/},"
    else
      deps_list+="${dep_sv_rel},"
    fi
  done < <(find "$proj_dir" -name '*.sv' \
           -not -path '*/dependencies/*' \
           -not -path '*/tests/*' \
           -not -path '*/tb/*' \
           -not -path '*/test/*' 2>/dev/null | sort -u)
  deps_list="${deps_list%,}"

  args=(-C "$proj_dir" -m "${top}:${sv_top}:${sv_rel}:${veryl_rel}")
  [[ -n "$deps_list" ]] && args+=(-p "$deps_list")
  [[ -n "$csv_out" ]] && args+=(-o "$csv_out")
  "$SCRIPT_DIR/compare.sh" "${args[@]}" || true
done < "$probe_log"

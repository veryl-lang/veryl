#!/usr/bin/env python3
"""Compare two CSVs produced by compare.sh (before vs after) and report how
yosys area/delay ratios moved for each module.

Usage:
    diff_bench.py <before.csv> <after.csv> [<before2.csv> <after2.csv> ...]

Prints per-module ratios and aggregate distribution statistics so the
effect of a synthesizer change across the whole benchmark set can be
judged at a glance.
"""

from __future__ import annotations
import argparse
import math
import sys
from pathlib import Path

import numpy as np
import pandas as pd


def load(paths: list[Path]) -> pd.DataFrame:
    frames = [pd.read_csv(p) for p in paths]
    df = pd.concat(frames, ignore_index=True)
    return df


def numeric(df: pd.DataFrame, col: str) -> pd.Series:
    """Coerce a column that may contain 'NA' / '-' / nan to float, leaving
    bad values as NaN."""
    return pd.to_numeric(df[col], errors="coerce")


def summarize(label: str, values: np.ndarray) -> None:
    v = values[~np.isnan(values) & (values > 0)]
    if len(v) == 0:
        print(f"{label:<24s}  (no data)")
        return
    logs = np.abs(np.log(v))
    print(f"{label:<24s}  n={len(v):>3d}  "
          f"mean={v.mean():.2f}  median={np.median(v):.2f}  "
          f"min={v.min():.2f}  max={v.max():.2f}  "
          f"p95={np.quantile(v, 0.95):.2f}  "
          f"|log| mean={logs.mean():.3f}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("csvs", nargs="+", type=Path,
                    help="pairs of (before, after) CSV paths")
    args = ap.parse_args()
    if len(args.csvs) % 2 != 0:
        print("error: arguments must come in (before, after) pairs",
              file=sys.stderr)
        return 1

    before_paths = args.csvs[0::2]
    after_paths = args.csvs[1::2]

    df_b = load(before_paths)
    df_a = load(after_paths)
    for df in (df_b, df_a):
        df["veryl_area"] = numeric(df, "veryl_area")
        df["yosys_area"] = numeric(df, "yosys_area")
        df["a_ratio"] = df["veryl_area"] / df["yosys_area"]
        df["veryl_delay"] = numeric(df, "veryl_delay")
        df["yosys_delay"] = numeric(df, "yosys_delay")
        # yosys_delay is already in ps (compare.sh convention); veryl in ns.
        df["d_ratio"] = (df["veryl_delay"] * 1000.0) / df["yosys_delay"]

    df_b_idx = df_b.set_index("module")
    df_a_idx = df_a.set_index("module")
    common = sorted(set(df_b_idx.index) & set(df_a_idx.index))

    print(f"# comparing {len(common)} modules present in both runs")
    print()
    print(f"{'module':<46s} {'a_ratio_before':>14s} {'a_ratio_after':>14s} {'Δa %':>7s}"
          f"   {'d_ratio_before':>14s} {'d_ratio_after':>14s} {'Δd %':>7s}")
    for m in common:
        ab = df_b_idx.loc[m, "a_ratio"]
        aa = df_a_idx.loc[m, "a_ratio"]
        db = df_b_idx.loc[m, "d_ratio"]
        da = df_a_idx.loc[m, "d_ratio"]
        # Percent change in the ratio itself — negative means the synth
        # moved closer to yosys (smaller ratio is better when we over-count;
        # larger closer to 1.0 when we under-count). We report the raw
        # fractional change of the ratio.
        da_pct = (aa / ab - 1) * 100 if (isinstance(ab, float) and ab > 0 and isinstance(aa, float) and aa > 0) else float("nan")
        dd_pct = (da / db - 1) * 100 if (isinstance(db, float) and db > 0 and isinstance(da, float) and da > 0) else float("nan")
        def fmt(x: float, prec: int = 2) -> str:
            return f"{x:>14.{prec}f}" if isinstance(x, float) and not math.isnan(x) else f"{'-':>14s}"
        def fmtpct(x: float) -> str:
            return f"{x:>+7.1f}" if isinstance(x, float) and not math.isnan(x) else f"{'-':>7s}"
        print(f"{m:<46s} {fmt(ab)} {fmt(aa)} {fmtpct(da_pct)}"
              f"   {fmt(db)} {fmt(da)} {fmtpct(dd_pct)}")

    print()
    print("aggregate ratio distributions (lower |log| is tighter, closer to 1.0 is more accurate)")
    ab_vals = df_b_idx.loc[common, "a_ratio"].to_numpy()
    aa_vals = df_a_idx.loc[common, "a_ratio"].to_numpy()
    db_vals = df_b_idx.loc[common, "d_ratio"].to_numpy()
    da_vals = df_a_idx.loc[common, "d_ratio"].to_numpy()
    summarize("area ratio: before", ab_vals)
    summarize("area ratio: after ", aa_vals)
    summarize("delay ratio: before", db_vals)
    summarize("delay ratio: after ", da_vals)

    return 0


if __name__ == "__main__":
    sys.exit(main())

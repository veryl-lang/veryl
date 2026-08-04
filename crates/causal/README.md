# veryl-causal

This crate contains IR-independent control-flow, SSA, MemorySSA, interval and
causal graph algorithms. A front end lowers one procedure into dense immutable
tables; the tables own no parser resources, symbol-table locks or Veryl IR
references.

The Veryl analyzer's user-visible acceptance bounds are specified in
[`crates/analyzer/COMBINATIONAL_LOOP_ANALYSIS.md`](../analyzer/COMBINATIONAL_LOOP_ANALYSIS.md).

The design has three constraints:

- Region endpoints form the version domain. Analysis cost depends on accesses,
  definitions, phis and CFG edges, not on a signal's declared bit width or an
  array's element count.
- Exact and unknown dependencies stay distinct. Only exact/proven paths are
  eligible for a hard combinational-loop diagnostic; dynamic aliases, external
  components, hierarchy, timed effects and unsupported syntax are retained as
  explicit incomplete reasons.
- Procedures are independent work items. Their summaries are deterministic and
  immutable, so callers may build them concurrently and merge them in source or
  module-topology order.

The procedure summary also supports sparse positional transfers. A copy or
concatenation relates source and destination spans, and access boundaries are
propagated through the transfer graph. This preserves bit provenance through
copy chains without expanding a wide vector into individual bits. The Veryl
adapter currently uses this for exact copies, concatenations, unary bitwise
not/plus, and equal-width pointwise bitwise operators. Four-state behavior and
context-determined widths are part of the eligibility check; unsupported
expressions retain their conservative all-to-all dependency.

The graph represents **structural dependence**, not Boolean functional
dependence and not `always_comb` sensitivity. LRM-defined bit placement may
split an operator into positional region transfers, but algebraic cancellation
does not remove an input edge: for example, `x & '0` remains structurally
dependent on the corresponding bits of `x`. Observer-only reads affect SV
sensitivity but do not define a signal value and therefore create no value
edge here.

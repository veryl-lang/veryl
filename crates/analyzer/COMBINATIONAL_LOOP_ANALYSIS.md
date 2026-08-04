# Combinational loop analysis

This document specifies the user-visible contract of Veryl's combinational
loop analysis. It defines the required and permitted acceptance sets, not a
particular analyzer IR, graph representation, or operator implementation.

The words **shall**, **shall not**, **may**, and **need not** are normative.
Examples explain the rules but do not add requirements.

## Purpose

Veryl diagnoses combinational feedback before a design is passed to a synthesis
tool. The analysis is synthesis-oriented. It is not a definition of
SystemVerilog event scheduling, `always_comb` sensitivity, or four-state
simulation behavior, and it need not reproduce the diagnostics or internal
netlist of a particular synthesis tool.

The contract deliberately leaves room for implementations to become more
precise. A compiler may reject a conservative feedback candidate which a more
precise compiler accepts. That latitude is bounded below: it cannot be used to
invent feedback outside the elaborated design.

## Terms and analysis domain

The **elaborated design** is the design after generate conditions, generate
loops, parameters, and concrete generic specializations have been resolved.
Declarations absent from the elaborated design do not participate in this
analysis.

An **object** is a signal, variable, port, function argument, function result,
or other value-bearing entity in the elaborated design. A **region** is some
part of an object, such as a packed bit range, struct field, or unpacked array
element. Region precision is an implementation choice unless this document
states otherwise.

A **dependency** is a directed causal relation from a read region to a written
region. Dependencies can arise from:

- a value used to compute the written value;
- a condition which controls whether or what value is written; or
- a value used to select the source or destination of an access.

The analysis covers combinational procedures, continuous assignments and
connections, combinational effects of called functions, and combinational
feedthrough across known module instances. Concurrent declarations have no
source ordering between them. Procedural ordering applies within one procedure
and through functions executed by that procedure.

State-holding and verification-only constructs do not create combinational
paths merely because they read and write the same objects. Clocked procedures
break a combinational path at the stored value. Initial, final, timed, and event
behavior is outside this analysis unless a combinational effect can be
established independently of that behavior.

## Required causal semantics

Analysis precision is optional; language ordering and connectivity are not.
Every implementation shall observe the following rules before applying the
acceptance latitude described below.

### Procedural order and reaching values

- A read before a write in the same procedure observes the value entering the
  procedure. A read after a write observes the latest reaching write on that
  path.
- A complete later write supersedes earlier writes which cannot reach a
  procedure exit or another observable effect. A dead, overwritten write shall
  not by itself form a loop.
- Branch joins, early exits, and loop exits shall preserve every definition
  which can reach the join. A write which may select only part of an object is
  a partial or weak write for the regions it does not certainly replace.
- Statements in distinct concurrent declarations shall not be ordered to make
  a cycle disappear.

These requirements concern the behavior being modeled. An implementation may
use SSA, MemorySSA, dataflow equations, direct netlist construction, or another
equivalent method.

### Value, control, and address use

An evaluated right-hand-side operand is a value dependency of the assignment.
A condition is a control dependency of writes whose execution or selected
value it controls. A selector is an address dependency of the read or write it
selects. An implementation may later prove that one of these conservative
dependencies is irrelevant.

A read performed only for observation, such as an argument of a display-like
operation, does not define a signal value and need not create a value
dependency. Side effects of functions evaluated by such an operation remain
ordinary procedural effects.

Implicit `always_comb` sensitivity is not a dependency graph. In particular,
sensitivity inclusion or exclusion does not create or remove a value, control,
or address dependency.

### Partial assignment and retained state

An unassigned region retains its entering value and may require an
incomplete-assignment or latch diagnostic. Retention alone is not a
combinational feedback edge. An explicit read of the entering value is a
dependency and can participate in a loop.

The same rule applies to a conditional without a covering arm, a zero-trip
loop, and a dynamic write which cannot certainly cover the whole destination.
A complete write before or after such a construct can remove retention when it
dominates every relevant exit.

For incomplete-assignment diagnostics, `cond_type(unique)`,
`cond_type(unique0)`, and `cond_type(priority)` suppress retention introduced
at the annotated conditional, while `cond_type(none)` does not. This policy
does not erase the retained value or any causal dependency, and retention
introduced at another unsuppressed control-flow join remains diagnostic.

### Functions

Dependencies through function inputs, outputs, return values, and
module-scope captures are part of the caller's combinational behavior. Writes
performed by a function occur at the call position for procedural-order
purposes. Function-local values participate only when they can affect a return,
an output, a capture, or another observable effect.

Concrete generic function specializations are distinct elaborated call
targets. An implementation may share their analysis, but it shall not transfer
a dependency from one specialization to another without a connection in the
elaborated design. A recursive call boundary which cannot be summarized is
incomplete; it shall not silently erase dependencies established before or
after the call.

### Modules and design boundaries

Known input-to-output feedthrough of a child module is a dependency in its
parent. Port mapping shall respect direction and the parent expressions and
destinations actually connected to the instance. Expressions used as input
actuals and output destination selectors follow the ordinary value, address,
and function-effect rules.

Concrete generic module specializations are distinct elaborated components. An
implementation may share their analysis, but dependencies shall be projected
through the specialization and actual connections being instantiated. A loop
inside a known child remains a loop whether or not a parent creates a return
path through that child.

A module definition may recursively instantiate a different concrete
specialization of itself. When elaboration produces a finite specialization
graph, every specialization and connection shall be analyzed normally; repeated
use of the same source-level module name is not an incomplete boundary. A cycle
which re-enters the same concrete specialization cannot form a finite
bottom-up summary and is incomplete unless rejected earlier as nonterminating
elaboration.

No return dependency is implied merely because a path reaches a module port or
the top of the design. The external environment shall not be modeled as an
implicit edge from an output back to an input.

SystemVerilog components, unknown external components, unresolved hierarchy,
and `inout` behavior are opaque unless their feedthrough is otherwise known.
An opaque boundary shall not be replaced by guessed feedthrough and used to
produce a hard combinational-loop error.

## Acceptance bounds

The **whole-dependency interpretation** is the conservative boundary. Within a
known expression or connection, it may treat every source region as affecting
every destination region. It still observes elaboration, procedural order,
dead overwrites, state boundaries, port direction, and actual connectivity. It
does not connect unrelated objects or cross an opaque boundary.

An implementation may accept additional programs by refining those
dependencies. No particular refinement is required. Subject to other Veryl
errors, a complete analysis shall satisfy these bounds:

- A program which is acyclic under the whole-dependency interpretation shall
  be accepted.
- A program whose known dependencies contain a cycle under every permitted
  refinement shall be rejected.
- A program which is cyclic only under a conservative interpretation may be
  accepted or rejected according to the precision of the implementation.

Equivalently:

`whole-dependency acyclic programs ⊆ accepted programs ⊆ programs without an unavoidable known-dependency cycle`

The middle category is deliberate. Such a program is not guaranteed to be
accepted by every analyzer version or implementation. Increasing precision
normally enlarges the accepted set without changing either bound.

This latitude applies independently to expressions, selectors, aggregates,
functions, and module summaries. It does not make the required causal semantics
above optional.

## Practical RTL acceptance

The acceptance bounds permit implementations with very different usefulness.
They are therefore not, by themselves, the policy of the default Veryl
analyzer. The default analyzer chooses a narrower operating point intended for
ordinary RTL development.

This policy is engineering judgment, not a consequence of the SystemVerilog
LRM. It accounts for all of the following:

- behavior commonly accepted by downstream synthesis tools;
- dependencies an RTL author can reasonably recognize from the source; and
- stable, predictable diagnostics which do not depend on a particular optimizer
  discovering a nonobvious identity.

The policy may therefore reject a technically loop-free construction when its
freedom from feedback depends on an unusually subtle value relationship. It
shall not use that judgment to reject ordinary static wiring which the required
precision below can distinguish. The policy may be revised as synthesis tools
and established RTL practice change.

### Baseline precision

The default policy shall preserve statically established placement through:

- direct copies and static packed or unpacked selections;
- struct fields, array elements, concatenations, aggregate constructors,
  literals, and statically repeated values;
- truncation, zero extension, sign extension, and width or sign casts whose
  result placement is statically determined;
- pointwise unary and bitwise operations, conditional result placement, and
  constant shifts, including vacated and sign-filled positions; and
- function arguments, function results and outputs, and known module port
  mappings when the corresponding placement crosses those boundaries.

The required placement composes through multiple such operations. An
implementation which collapses all of these cases to whole-object dependency
does not implement the default policy, even if it remains within the wider
acceptance bounds.

Operators and mappings not covered by this baseline may use whole dependency.
For example, the default policy does not require bit-prefix modeling for
addition or subtraction, or value-range modeling for a dynamic selector.

### Practical upper bound

The default analyzer may use information beyond the baseline without
requiring that every analyzer start from the same unoptimized representation.
However, a refinement beyond the baseline shall be the sole reason for accepting
a program only when at least one of the following is true:

- the refinement has been adopted by the default policy based on downstream
  interoperability and ordinary RTL practice; or
- the transformation which removes the dependency is materialized in the
  representation emitted to downstream tools.

For example, an optimizer may prove that correlated operands produce a
constant. The default analyzer may rely on that fact for acceptance when the
emitted design contains the resulting constant or an equivalent form without
that dependency. It shall not rely on the proof while emitting the original
expression and requiring every downstream tool to rediscover it.

The same rule applies to value ranges, relationships between selectors,
algebraic cancellation, and operator-specific precision beyond the baseline.
These facts may always be used for performance, diagnostics, or to construct the
emitted representation; the restriction concerns using an unmaterialized fact
as the final reason to suppress a hard error.

Tool-specific or experimental analysis modes may choose a different point
within the general acceptance bounds, but shall not silently replace the
default policy. Changes to the default policy which can turn a previously
accepted design into an error are compatibility changes, even when both
behaviors fall within the wider language bounds.

## Permitted precision

Within the wider acceptance bounds, an implementation may use any fact which is
valid for the representation it analyzes. It may analyze optimized or
unoptimized input. The default analyzer additionally observes the practical RTL
policy above. An implementation may, but need not, use:

- packed bit positions and unpacked element positions;
- constant values, unreachable control-flow paths, and short-circuit facts;
- value ranges and relationships between selector expressions;
- relationships between operands or between branches of an expression;
- width, signedness, extension, truncation, and aggregate layout;
- facts introduced or retained by earlier optimization and lowering; and
- operator-specific dependencies.

No algebraic simplification, value-range analysis, or correlation analysis is
required. Conversely, this contract does not require the analyzer to discard
such facts before loop detection.

Refinement means using valid information to omit a conservative candidate. It
does not permit a known causal effect to be dropped merely because the
implementation has no model for it. Such a gap is incomplete or an
implementation defect, depending on whether the construct is opaque or is part
of the supported analyzer IR.

Examples of permitted choices include:

- Addition and subtraction may be modeled as whole-result dependencies. An
  implementation may instead recognize that a result bit is independent of
  some higher operand bits.
- A constant shift may be treated conservatively, or its shifted positions,
  vacated positions, and sign fill may be tracked separately.
- Casts, concatenations, aggregate constructors, and repeated values may use
  their statically known bit or element placement.
- A dynamic selector may conservatively cover its enclosing object. A proven
  value range or relationship between selectors may narrow that set.
- Identical operands or branches may remain structurally dependent, or an
  optimizer may establish that their contribution cancels or is constant.

The result of a refinement may remove a conservative loop candidate. It shall
not justify an edge between source and destination objects which are not
connected by the elaborated program.

## Loops and control flow

Finite source loops have the procedural meaning of their iterations, including
zero iterations and `break`. An implementation may expand them, summarize
them, or use facts about iterator values. It need not use the same strategy or
limit for every loop form.

If a loop cannot be analyzed within an implementation limit, the result is
incomplete for the affected behavior. Known dependencies found in the loop or
the surrounding procedure remain usable. Treating an unanalyzed loop as always
zero-trip or silently dropping all of its effects is not a complete result.

The same rule applies when an implementation declines to evaluate a bound
which could in principle be evaluated. Resource limits may reduce completeness;
they shall not create guessed edges outside the whole-dependency boundary.

## Incomplete analysis

An analysis is **incomplete** when some relevant dependency cannot be placed
within the acceptance bounds. Causes include opaque boundaries, `inout`,
unresolved hierarchy, unresolved recursive calls or cyclic concrete
specialization graphs, unevaluated generic shapes, unanalyzed loops, timed or
event effects, and legal constructs not yet represented by the analyzer.

A dynamic access is not incomplete merely because its exact selected region is
unknown. If its object or longest static prefix has a known shape, whole
dependency over that bounded region is conservative and complete. The access is
incomplete only when the enclosing region itself cannot be bounded or when its
mapping is lost across another incomplete boundary.

The acceptance equation above applies to the portion for which analysis is
complete. Incomplete behavior has these rules:

- Unknown behavior shall not be promoted to guessed feedthrough for a hard
  loop error.
- A known cycle shall still be reported even when unrelated or adjacent
  behavior is incomplete.
- Known dependencies shall not be discarded merely because the same procedure
  or module also contains an incomplete effect.
- An implementation may expose incompleteness separately from ordinary loop
  diagnostics. Absence of a loop error is not proof of acyclicity when the
  result is incomplete.
- Failure to represent otherwise valid analyzer IR is an implementation defect,
  not a new user-language restriction.

## Diagnostics

A hard combinational-loop diagnostic shall be backed by a directed cycle in
the dependency interpretation selected by the implementation. The diagnostic
shall identify a simple source-backed cycle rather than list unrelated members
of a larger strongly connected component. It need not find the globally
shortest possible cycle.

Multiple cycles in one strongly connected component may be represented by one
diagnostic. For identical source, configuration, and analysis precision, cycle
selection and diagnostic ordering shall be deterministic.

An incomplete result does not weaken a simultaneously established cycle. When
both exist, the hard diagnostic is issued for the established cycle and the
unknown behavior remains incomplete.

## Classification examples

- A value assigned directly or transitively from itself with no intervening
  state boundary has an unavoidable cycle and is rejected.
- Two concurrent combinational declarations which drive `a` from `b` and `b`
  from `a` have an unavoidable cycle. Source order cannot make them acyclic.
- A self-dependent temporary write which is completely overwritten before it
  can affect a procedure exit or side effect does not form a loop.
- A partial assignment which never reads the previous value is a state-coverage
  problem, not by itself a combinational loop.
- Feedback which appears only because addition, a cast, or a dynamic selector
  was modeled as a whole dependency lies in the implementation-precision
  category. A more precise implementation may accept it.
- A possible return path through an opaque external component is incomplete,
  not a hard loop. A separate known cycle in the same module is still rejected.

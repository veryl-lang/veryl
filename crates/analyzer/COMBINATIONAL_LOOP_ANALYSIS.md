# Combinational loop analysis

This document specifies the user-visible acceptance bounds of Veryl's
combinational loop analysis. It intentionally does not prescribe an analyzer
IR, a graph representation, or the precision of individual operator models.

## Purpose

Veryl diagnoses combinational feedback before a design is passed to a synthesis
tool. The analysis is synthesis-oriented. It is not a definition of
SystemVerilog event scheduling, `always_comb` sensitivity, or four-state
simulation behavior, and it is not required to reproduce the diagnostics of a
particular synthesis tool.

The analysis considers value, control, and address dependencies in the
elaborated design. Procedural statement order, overwrites, branch exits,
function calls, and module connections are part of that design and are not
optional sources of analysis precision.

## Acceptance bounds

An implementation may model an evaluated operand conservatively as a dependency
of the whole result. This whole-dependency interpretation is the conservative
boundary of the analysis. It applies only to operands, results, and connections
that exist in the elaborated design; it does not permit dependencies to be
invented between otherwise disconnected objects or across an opaque boundary.

An implementation may accept additional programs by using more precise
information. No particular refinement is required. In particular, an
implementation may, but need not, use bit positions, constant values, value
ranges, relationships between operands, or facts established by optimization
and lowering.

Subject to other Veryl errors, the following requirements define the permitted
acceptance range:

- A program that is acyclic under the whole-dependency interpretation shall be
  accepted.
- A program whose known dependencies contain a cycle under every permitted
  refinement shall be rejected.
- A program that is cyclic only under a conservative interpretation may be
  accepted or rejected according to the precision of the implementation.

Equivalently, for the combinational-loop rule alone:

`whole-dependency acyclic programs ⊆ accepted programs ⊆ programs without an unavoidable known-dependency cycle`

The third category is deliberate. Such a program is not guaranteed to be
accepted by every analyzer version or implementation. Increasing analysis
precision should normally move programs from this category into the accepted
set; it need not change the language rule or the conservative boundary.

The latitude above concerns the precision of dependencies inside known
constructs. It does not make procedural semantics implementation-defined. A
later complete assignment, for example, removes an earlier reaching definition
when required by the language's statement ordering. Likewise, an implementation
that cannot model a function or module connection shall treat that connection
as incomplete rather than silently omit it from an otherwise complete result.

## State and incomplete boundaries

Retention caused by an incomplete assignment is state, not by itself a
combinational feedback edge. It is handled by the corresponding assignment or
latch diagnostic. An explicit read of the previous value is still a dependency
and can participate in a combinational loop.

If the behavior of a boundary cannot be established, the analysis may report an
incomplete result. Examples include opaque SystemVerilog components, `inout`
connections, unresolved hierarchy, recursive boundaries, and effects which
cannot be represented by the analyzer. An opaque boundary alone shall not be
replaced by guessed feedthrough and used to produce a hard combinational-loop
error. Incomplete information does not suppress a loop established entirely by
known dependencies elsewhere in the design.

## Examples of permitted precision

These examples illustrate implementation choices; they are not additional
requirements.

- Addition and subtraction may be modeled as whole-result dependencies. An
  implementation may instead recognize that a result bit is independent of
  some higher operand bits and thereby accept a bit-disjoint feedback candidate.
- A constant shift may be treated conservatively, or its shifted positions,
  vacated positions, and sign fill may be tracked separately.
- Casts, concatenations, aggregate constructors, and repeated values may use
  their statically known bit or element placement.
- A dynamic selector may conservatively cover its enclosing object. An
  implementation may use a proven value range or a relationship between
  selector expressions to narrow that set.
- Algebraic relationships between operands, identical branches, and facts
  introduced by an optimizer may remove dependencies. An implementation is not
  required to discover or preserve those facts for this analysis.

These choices affect only programs between the required-acceptance and
required-rejection bounds. They cannot justify a dependency outside the
elaborated design or turn an unknown external effect into a proven loop.

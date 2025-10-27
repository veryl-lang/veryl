use crate::multi_sources::{MultiSources, Source};
use miette::{self, Diagnostic, Severity, SourceSpan};
use std::fmt;
use thiserror::Error;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::TokenSource;

#[derive(Error, Diagnostic, Debug, PartialEq, Eq)]
pub enum AnalyzerError {
    #[diagnostic(severity(Error), code(anonymous_identifier_usage), help(""), url(""))]
    #[error("Anonymous identifier can't be placed at here")]
    AnonymousIdentifierUsage {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(call_non_function),
        help("remove call to non-function symbol"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#call_non_function"
        )
    )]
    #[error("Calling non-function symbol \"{identifier}\" which has kind \"{kind}\"")]
    CallNonFunction {
        identifier: String,
        kind: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        #[label(collection, "instantiated at")]
        inst_context: Vec<SourceSpan>,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(cyclic_type_dependency),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#cyclic_type_dependency"
        )
    )]
    #[error("Cyclic dependency between {start} and {end}")]
    CyclicTypeDependency {
        start: String,
        end: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(duplicated_identifier),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#duplicated_identifier"
        )
    )]
    #[error("{identifier} is duplicated")]
    DuplicatedIdentifier {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(multiple_assignment),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#multiple_assignment"
        )
    )]
    #[error("{identifier} is assigned in multiple procedural blocks or assignment statements")]
    MultipleAssignment {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label(collection, "Assigned")]
        error_locations: Vec<SourceSpan>,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_assignment),
        help("remove the assignment"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_assignment"
        )
    )]
    #[error("{identifier} can't be assigned because it is {kind}")]
    InvalidAssignment {
        identifier: String,
        kind: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_assignment_to_const),
        help("remove the assignment"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_assignment_to_const"
        )
    )]
    #[error("{identifier} can't be assigned because it is const")]
    InvalidAssignmentToConst {
        identifier: String,
        kind: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(invalid_connect_operand), help(""), url(""))]
    #[error("{identifier} can't be used as a connect operand because {reason}")]
    InvalidConnectOperand {
        identifier: String,
        reason: String,
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_modifier),
        help("remove the modifier"),
        url("")
    )]
    #[error("{kind} modifier can't be used because {reason}")]
    InvalidModifier {
        kind: String,
        reason: String,
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_direction),
        help("remove {kind} direction"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_direction"
        )
    )]
    #[error("{kind} direction can't be placed at here")]
    InvalidDirection {
        kind: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_operand),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_operand")
    )]
    #[error("{kind} cannot be used as a operand of {op} operator")]
    InvalidOperand {
        kind: String,
        op: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        // TODO
        //#[label(collection, "instantiated at")]
        //inst_context: Vec<SourceSpan>,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(invalid_logical_operand),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_operand")
    )]
    #[error("{kind} should be 1-bit value")]
    InvalidLogicalOperand {
        kind: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_factor),
        help("remove {kind} from expression"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_factor")
    )]
    #[error("{} cannot be used as a factor in an expression", match .identifier {
        Some(x) => format!("{} of \"{}\"", x, .kind),
        None => format!("{}", .kind),
    })]
    InvalidFactor {
        identifier: Option<String>,
        kind: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        #[label(collection, "instantiated at")]
        inst_context: Vec<SourceSpan>,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(invalid_select),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_select")
    )]
    #[error("invalid select caused by {kind}")]
    InvalidSelect {
        kind: InvalidSelectKind,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        #[label(collection, "instantiated at")]
        inst_context: Vec<SourceSpan>,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(invalid_identifier),
        help("follow naming rule"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_identifier"
        )
    )]
    #[error("{identifier} violate \"{rule}\" naming rule")]
    InvalidIdentifier {
        identifier: String,
        rule: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_import),
        help("fix import item"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_import")
    )]
    #[error("This item can't be imported")]
    InvalidImport {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_lsb),
        help("remove lsb"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_lsb")
    )]
    #[error("lsb can't be placed at here")]
    InvalidLsb {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_msb),
        help("remove msb"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_msb")
    )]
    #[error("msb can't be placed at here")]
    InvalidMsb {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_number_character),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_number_character"
        )
    )]
    #[error("{kind} number can't contain {cause}")]
    InvalidNumberCharacter {
        cause: char,
        kind: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_statement),
        help("remove {kind} statement"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_statement"
        )
    )]
    #[error("{kind} statement can't be placed at here")]
    InvalidStatement {
        kind: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_clock),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_clock")
    )]
    #[error(
        "#{identifier} can't be used as a clock because it is not 'clock' type nor a single bit signal"
    )]
    InvalidClock {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(multiple_default_clock), help(""), url(""))]
    #[error(
        "{identifier} can't be used as the default clock because the default clock has already been specified."
    )]
    MultipleDefaultClock {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_modport_variable_item),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_modport_variable_item"
        )
    )]
    #[error("#{identifier} is not a variable")]
    InvalidModportVariableItem {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_modport_function_item),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_modport_function_item"
        )
    )]
    #[error("#{identifier} is not a function")]
    InvalidModportFunctionItem {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(unexpandable_modport), help(""), url(""))]
    #[error("#{identifier} can't be expanded")]
    UnexpandableModport {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(invalid_port_default_value), help(""), url(""))]
    #[error("{kind}")]
    InvalidPortDefaultValue {
        kind: InvalidPortDefaultValueKind,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_reset),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_reset")
    )]
    #[error(
        "#{identifier} can't be used as a reset because it is not 'reset' type nor a single bit signal"
    )]
    InvalidReset {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(multiple_default_reset), help(""), url(""))]
    #[error(
        "{identifier} can't be used as the default reset because the default reset has already been specified."
    )]
    MultipleDefaultReset {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_reset_non_elaborative),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_reset_non_elaborative"
        )
    )]
    #[error("Reset-value cannot be used because it is not evaluable at elaboration time")]
    InvalidResetNonElaborative {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_case_condition_non_elaborative),
        help(""),
        url("")
    )]
    #[error("Case condition value cannot be used because it is not evaluable at elaboration time")]
    InvalidCaseConditionNonElaborative {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_const_non_elaborative),
        help(""),
        url("")
    )]
    #[error("Const value cannot be used because it is not evaluable at elaboration time")]
    InvalidConstNonElaborative {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(invalid_cast), help(""), url(""))]
    #[error("Casting from {from} to {to} is incompatible")]
    InvalidCast {
        from: String,
        to: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_test),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#invalid_test")
    )]
    #[error("test is invalid because {cause}")]
    InvalidTest {
        cause: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(invalid_type_declaration), help(""), url(""))]
    #[error("{kind} can't be declared in interface declaration")]
    InvalidTypeDeclaration {
        kind: String,
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(incompat_proto),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#incompat_proto")
    )]
    #[error("{identifier} is incompatible with {proto} because {cause}")]
    IncompatProto {
        identifier: String,
        proto: String,
        cause: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(missing_default_argument),
        help("give default argument"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#missing_default_argument"
        )
    )]
    #[error("missing default argument for parameter {identifier}")]
    MissingDefaultArgument {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(mismatch_function_arity),
        help("fix function arguments"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#mismatch_function_arity"
        )
    )]
    #[error("function \"{name}\" has {arity} arguments, but {args} arguments are supplied")]
    MismatchFunctionArity {
        name: String,
        arity: usize,
        args: usize,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(mismatch_function_arg),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#mismatch_function_arg"
        )
    )]
    #[error("\"{src}\" type can't be used as an argument of function \"{name}\"")]
    MismatchFunctionArg {
        name: String,
        src: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(mismatch_generics_arity),
        help("fix generics arguments"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#mismatch_generics_arity"
        )
    )]
    #[error("generics \"{name}\" has {arity} generic arguments, but {args} arguments are supplied")]
    MismatchGenericsArity {
        name: String,
        arity: usize,
        args: usize,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(mismatch_attribute_args),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#mismatch_attribute_args"
        )
    )]
    #[error("Arguments of \"{name}\" is expected to \"{expected}\"")]
    MismatchAttributeArgs {
        name: String,
        expected: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(mismatch_type),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#mismatch_type")
    )]
    #[error("\"{name}\" is expected to \"{expected}\", but it is \"{actual}\"")]
    MismatchType {
        name: String,
        expected: String,
        actual: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(mismatch_clock_domain),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#mismatch_clock_domain"
        )
    )]
    #[error("Clock domain crossing is detected")]
    MismatchClockDomain {
        clock_domain: String,
        other_domain: String,
        #[source_code]
        input: MultiSources,
        #[label("clock domain {clock_domain}")]
        error_location: SourceSpan,
        #[label("clock domain {other_domain}")]
        other_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(mismatch_assignment),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#mismatch_assignment"
        )
    )]
    #[error("\"{src}\" can't be assigned to \"{dst}\"")]
    MismatchAssignment {
        src: String,
        dst: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        #[label(collection, "instantiated at")]
        inst_context: Vec<SourceSpan>,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(missing_if_reset),
        help("add if_reset statement"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#missing_if_reset")
    )]
    #[error("if_reset statement is required for always_ff with reset signal")]
    MissingIfReset {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(missing_port),
        help("add \"{port}\" port"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#missing_port")
    )]
    #[error("module \"{name}\" has \"{port}\", but it is not connected")]
    MissingPort {
        name: String,
        port: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(missing_clock_signal),
        help("add clock port"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#missing_clock_signal"
        )
    )]
    #[error("clock signal is required for always_ff statement")]
    MissingClockSignal {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(missing_reset_signal),
        help("add reset port"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#missing_reset_signal"
        )
    )]
    #[error("reset signal is required for always_ff with if_reset statement")]
    MissingResetSignal {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(missing_reset_statement),
        help("add reset statement"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#missing_reset_statement"
        )
    )]
    #[error("{name} is not reset in if_reset statement")]
    MissingResetStatement {
        name: String,
        #[source_code]
        input: MultiSources,
        #[label(collection, "Not reset")]
        error_locations: Vec<SourceSpan>,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(missing_tri),
        help("add tri type modifier"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#missing_tri")
    )]
    #[error("tri type modifier is required at inout port")]
    MissingTri {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(missing_clock_domain),
        help("add clock domain annotation"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#missing_clock_domain"
        )
    )]
    #[error("clock domain annotation is required when there are multiple clocks")]
    MissingClockDomain {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(mixed_function_argument),
        help("fix function arguments"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#mixed_function_argument"
        )
    )]
    #[error(
        "positional arguments and named arguments are mixed. Both of them can't be used at the same time"
    )]
    MixedFunctionArgument {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(sv_keyword_usage),
        help("Change the identifier to a non-SystemVerilog keyword"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#sv_keyword_usage")
    )]
    #[error("SystemVerilog keyword may not be used as identifier")]
    SvKeywordUsage {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(sv_with_implicit_reset),
        help("Use types with explicit synchronisity and polarity like `reset_async_low`"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#sv_with_implicit_reset"
        )
    )]
    #[error(
        "Reset type with implicit synchronisity and polarity can't be connected to SystemVerilog module"
    )]
    SvWithImplicitReset {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_clock_domain),
        help("Remove the clock domain annotation"),
        url("")
    )]
    #[error("Cannot specify clock domain annotation to module instance")]
    InvalidClockDomain {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(too_large_enum_variant),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#too_large_enum_variant"
        )
    )]
    #[error(
        "The value of enum variant {identifier} is {value}, it is can't be represented by {width} bits"
    )]
    TooLargeEnumVariant {
        identifier: String,
        value: isize,
        width: usize,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unevaluatable_enum_variant_value),
        help(""),
        url("")
    )]
    #[error("The value of enum variant {identifier} cannot be evaluated")]
    UnevaluatableEnumVariant {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(invalid_enum_variant_value), help(""), url(""))]
    #[error("The value of enum variant {identifier} is not encoded value by {encoding}")]
    InvalidEnumVariant {
        identifier: String,
        encoding: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(too_large_number),
        help("increase bit width"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#too_large_number")
    )]
    #[error("number is over the maximum size of {width} bits")]
    TooLargeNumber {
        width: usize,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(too_much_enum_variant),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#too_much_enum_variant"
        )
    )]
    #[error(
        "enum {identifier} has {number} variants, they are can't be represented by {width} bits"
    )]
    TooMuchEnumVariant {
        identifier: String,
        number: usize,
        width: usize,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(invisible_identifier), help(""), url(""))]
    #[error("cannot refer indentifier {identifier} because it is invisible at here")]
    InvisibleIndentifier {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(undefined_identifier),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#undefined_identifier"
        )
    )]
    #[error("{identifier} is undefined")]
    UndefinedIdentifier {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(referring_before_definition),
        help("move definition before reference point"),
        url("")
    )]
    #[error("{identifier} is referred before it is defined.")]
    ReferringBeforeDefinition {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unresolvable_generic_argument),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unresolvable_generic_argument"
        )
    )]
    #[error("{identifier} can't be resolved from the definition of generics")]
    UnresolvableGenericArgument {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        #[label("Definition")]
        definition_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unknown_attribute),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unknown_attribute"
        )
    )]
    #[error("\"{name}\" is not valid attribute")]
    UnknownAttribute {
        name: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(invalid_enbed), help(""), url(""))]
    #[error("embed (way: {way}/lang: {lang}) can't be used at here")]
    InvalidEmbed {
        way: String,
        lang: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(invalid_enbed_identifier), help(""), url(""))]
    #[error("embed identifier can be used in way: inline/lang: sv code block only")]
    InvalidEmbedIdentifier {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unknown_embed_lang),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unknown_embed_lang"
        )
    )]
    #[error("\"{name}\" is not valid embed language")]
    UnknownEmbedLang {
        name: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unknown_embed_way),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unknown_embed_way"
        )
    )]
    #[error("\"{name}\" is not valid embed way")]
    UnknownEmbedWay {
        name: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unknown_include_way),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unknown_include_way"
        )
    )]
    #[error("\"{name}\" is not valid include way")]
    UnknownIncludeWay {
        name: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unknown_member),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unknown_member")
    )]
    #[error("\"{name}\" doesn't have member \"{member}\"")]
    UnknownMember {
        name: String,
        member: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unknown_unsafe),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unknown_unsafe")
    )]
    #[error("\"{name}\" is not valid unsafe identifier")]
    UnknownUnsafe {
        name: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(private_member),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#private_member")
    )]
    #[error("\"{name}\" is private member")]
    PrivateMember {
        name: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(private_namespace), help(""), url(""))]
    #[error("\"{name}\" is private namespace")]
    PrivateNamespace {
        name: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unknown_msb),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unknown_msb")
    )]
    #[error("resolving msb is failed")]
    UnknownMsb {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unknown_port),
        help("remove \"{port}\" port"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unknown_port")
    )]
    #[error("module \"{name}\" doesn't have port \"{port}\", but it is connected")]
    UnknownPort {
        name: String,
        port: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unknown_param),
        help("remove \"{param}\" param"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unknown_param")
    )]
    #[error("module \"{name}\" doesn't have param \"{param}\", but it is overrided")]
    UnknownParam {
        name: String,
        param: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(unenclosed_inner_if_expression),
        help("enclose the inner if expression in parenthesis"),
        url("")
    )]
    #[error("inner if expression should be enclosed in parenthesis, but is not")]
    UnenclosedInnerIfExpression {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(unused_variable),
        help("add prefix `_` to unused variable name"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unused_variable")
    )]
    #[error("{identifier} is unused")]
    UnusedVariable {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(unused_return),
        help("add variable assignment for function return"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unused_return")
    )]
    #[error("return value of {identifier} is unused")]
    UnusedReturn {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(unassign_variable),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unassign_variable"
        )
    )]
    #[error("{identifier} is unassigned")]
    UnassignVariable {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unassignable_output),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unassignable_output"
        )
    )]
    #[error("unassignable type is connected to output port")]
    UnassignableOutput {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(uncovered_branch),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#uncovered_branch")
    )]
    #[error("{identifier} is not covered by all branches, it causes latch generation")]
    UncoveredBranch {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label(collection, "Covered")]
        error_locations: Vec<SourceSpan>,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(reserved_identifier),
        help("prefix `__` can't be used"),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#reserved_identifier"
        )
    )]
    #[error("{identifier} is reverved for compiler usage")]
    ReservedIdentifier {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(include_failure),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#include_failure")
    )]
    #[error("\"{name}\" can't be read because \"{cause}\"")]
    IncludeFailure {
        name: String,
        cause: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(wrong_seperator),
        help("replace valid separator \"{valid_separator}\""),
        url("")
    )]
    #[error("separator \"{separator}\" can't be used at here")]
    WrongSeparator {
        separator: String,
        valid_separator: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(infinite_recursion), help(""), url(""))]
    #[error("infinite instance recursion is detected")]
    InfiniteRecursion {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(exceed_limit), help(""), url(""))]
    #[error("exceed {kind} limit: {value}")]
    ExceedLimit {
        kind: String,
        value: usize,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(ambiguous_elsif),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#ambiguous_elsif")
    )]
    #[error("elsif/else attribute is ambiguous because {cause}")]
    AmbiguousElsif {
        cause: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(last_item_with_define),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#last_item_with_define"
        )
    )]
    #[error(
        "ifdef/ifndef/elsif/else attribute can't be used with the last item in comma-separated list"
    )]
    LastItemWithDefine {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(unsigned_loop_variable_in_descending_order_for_loop),
        help("use singed type as loop variable"),
        url("")
    )]
    #[error("use of unsigned loop variable in descending order for loop may cause infinite loop")]
    UnsignedLoopVariableInDescendingOrderForLoop {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(fixed_type_with_signed_modifier),
        help("remove 'signed` modifier"),
        url("")
    )]
    #[error("'signed' modifier can't be used with fixed type")]
    FixedTypeWithSignedModifier {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unsupported_by_ir),
        help(""),
        url(
            "https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#unsupported_by_ir"
        )
    )]
    #[error("This description is not supported by IR @ {code}")]
    UnsupportedByIr {
        code: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },
}

fn source(token: &TokenRange) -> MultiSources {
    let path = token.beg.source.to_string();
    let text = token.beg.source.get_text();
    MultiSources {
        sources: vec![Source { path, text }],
    }
}

fn source_with_context(
    token: &TokenRange,
    context: &[TokenRange],
) -> (MultiSources, Vec<SourceSpan>) {
    let path = token.beg.source.to_string();
    let text = token.beg.source.get_text();

    let mut base = text.len();
    let mut ranges = Vec::new();
    let mut sources = Vec::new();

    sources.push(Source { path, text });

    for x in context.iter().rev() {
        let path = x.beg.source.to_string();
        let text = x.beg.source.get_text();

        let mut range = *x;
        range.offset(base as u32);
        ranges.push(range.into());

        base += text.len();

        sources.push(Source { path, text });
    }

    let sources = MultiSources { sources };
    (sources, ranges)
}

impl AnalyzerError {
    pub fn is_error(&self) -> bool {
        matches!(self.severity(), Some(Severity::Error) | None)
    }

    pub fn token_source(&self) -> TokenSource {
        match self {
            AnalyzerError::AnonymousIdentifierUsage { token_source, .. } => *token_source,
            AnalyzerError::CallNonFunction { token_source, .. } => *token_source,
            AnalyzerError::CyclicTypeDependency { token_source, .. } => *token_source,
            AnalyzerError::DuplicatedIdentifier { token_source, .. } => *token_source,
            AnalyzerError::MultipleAssignment { token_source, .. } => *token_source,
            AnalyzerError::InvalidAssignment { token_source, .. } => *token_source,
            AnalyzerError::InvalidAssignmentToConst { token_source, .. } => *token_source,
            AnalyzerError::InvalidConnectOperand { token_source, .. } => *token_source,
            AnalyzerError::InvalidModifier { token_source, .. } => *token_source,
            AnalyzerError::InvalidDirection { token_source, .. } => *token_source,
            AnalyzerError::InvalidOperand { token_source, .. } => *token_source,
            AnalyzerError::InvalidLogicalOperand { token_source, .. } => *token_source,
            AnalyzerError::InvalidFactor { token_source, .. } => *token_source,
            AnalyzerError::InvalidSelect { token_source, .. } => *token_source,
            AnalyzerError::InvalidIdentifier { token_source, .. } => *token_source,
            AnalyzerError::InvalidImport { token_source, .. } => *token_source,
            AnalyzerError::InvalidLsb { token_source, .. } => *token_source,
            AnalyzerError::InvalidMsb { token_source, .. } => *token_source,
            AnalyzerError::InvalidNumberCharacter { token_source, .. } => *token_source,
            AnalyzerError::InvalidStatement { token_source, .. } => *token_source,
            AnalyzerError::InvalidClock { token_source, .. } => *token_source,
            AnalyzerError::MultipleDefaultClock { token_source, .. } => *token_source,
            AnalyzerError::InvalidModportVariableItem { token_source, .. } => *token_source,
            AnalyzerError::InvalidModportFunctionItem { token_source, .. } => *token_source,
            AnalyzerError::UnexpandableModport { token_source, .. } => *token_source,
            AnalyzerError::InvalidPortDefaultValue { token_source, .. } => *token_source,
            AnalyzerError::InvalidReset { token_source, .. } => *token_source,
            AnalyzerError::MultipleDefaultReset { token_source, .. } => *token_source,
            AnalyzerError::InvalidResetNonElaborative { token_source, .. } => *token_source,
            AnalyzerError::InvalidCaseConditionNonElaborative { token_source, .. } => *token_source,
            AnalyzerError::InvalidConstNonElaborative { token_source, .. } => *token_source,
            AnalyzerError::InvalidCast { token_source, .. } => *token_source,
            AnalyzerError::InvalidTest { token_source, .. } => *token_source,
            AnalyzerError::InvalidTypeDeclaration { token_source, .. } => *token_source,
            AnalyzerError::IncompatProto { token_source, .. } => *token_source,
            AnalyzerError::MissingDefaultArgument { token_source, .. } => *token_source,
            AnalyzerError::MismatchFunctionArity { token_source, .. } => *token_source,
            AnalyzerError::MismatchFunctionArg { token_source, .. } => *token_source,
            AnalyzerError::MismatchGenericsArity { token_source, .. } => *token_source,
            AnalyzerError::MismatchAttributeArgs { token_source, .. } => *token_source,
            AnalyzerError::MismatchType { token_source, .. } => *token_source,
            AnalyzerError::MismatchClockDomain { token_source, .. } => *token_source,
            AnalyzerError::MismatchAssignment { token_source, .. } => *token_source,
            AnalyzerError::MissingIfReset { token_source, .. } => *token_source,
            AnalyzerError::MissingPort { token_source, .. } => *token_source,
            AnalyzerError::MissingClockSignal { token_source, .. } => *token_source,
            AnalyzerError::MissingResetSignal { token_source, .. } => *token_source,
            AnalyzerError::MissingResetStatement { token_source, .. } => *token_source,
            AnalyzerError::MissingTri { token_source, .. } => *token_source,
            AnalyzerError::MissingClockDomain { token_source, .. } => *token_source,
            AnalyzerError::MixedFunctionArgument { token_source, .. } => *token_source,
            AnalyzerError::SvKeywordUsage { token_source, .. } => *token_source,
            AnalyzerError::SvWithImplicitReset { token_source, .. } => *token_source,
            AnalyzerError::InvalidClockDomain { token_source, .. } => *token_source,
            AnalyzerError::TooLargeEnumVariant { token_source, .. } => *token_source,
            AnalyzerError::UnevaluatableEnumVariant { token_source, .. } => *token_source,
            AnalyzerError::InvalidEnumVariant { token_source, .. } => *token_source,
            AnalyzerError::TooLargeNumber { token_source, .. } => *token_source,
            AnalyzerError::TooMuchEnumVariant { token_source, .. } => *token_source,
            AnalyzerError::InvisibleIndentifier { token_source, .. } => *token_source,
            AnalyzerError::UndefinedIdentifier { token_source, .. } => *token_source,
            AnalyzerError::ReferringBeforeDefinition { token_source, .. } => *token_source,
            AnalyzerError::UnresolvableGenericArgument { token_source, .. } => *token_source,
            AnalyzerError::UnknownAttribute { token_source, .. } => *token_source,
            AnalyzerError::InvalidEmbed { token_source, .. } => *token_source,
            AnalyzerError::InvalidEmbedIdentifier { token_source, .. } => *token_source,
            AnalyzerError::UnknownEmbedLang { token_source, .. } => *token_source,
            AnalyzerError::UnknownEmbedWay { token_source, .. } => *token_source,
            AnalyzerError::UnknownIncludeWay { token_source, .. } => *token_source,
            AnalyzerError::UnknownMember { token_source, .. } => *token_source,
            AnalyzerError::UnknownUnsafe { token_source, .. } => *token_source,
            AnalyzerError::PrivateMember { token_source, .. } => *token_source,
            AnalyzerError::PrivateNamespace { token_source, .. } => *token_source,
            AnalyzerError::UnknownMsb { token_source, .. } => *token_source,
            AnalyzerError::UnknownPort { token_source, .. } => *token_source,
            AnalyzerError::UnknownParam { token_source, .. } => *token_source,
            AnalyzerError::UnenclosedInnerIfExpression { token_source, .. } => *token_source,
            AnalyzerError::UnusedVariable { token_source, .. } => *token_source,
            AnalyzerError::UnusedReturn { token_source, .. } => *token_source,
            AnalyzerError::UnassignVariable { token_source, .. } => *token_source,
            AnalyzerError::UnassignableOutput { token_source, .. } => *token_source,
            AnalyzerError::UncoveredBranch { token_source, .. } => *token_source,
            AnalyzerError::ReservedIdentifier { token_source, .. } => *token_source,
            AnalyzerError::IncludeFailure { token_source, .. } => *token_source,
            AnalyzerError::WrongSeparator { token_source, .. } => *token_source,
            AnalyzerError::InfiniteRecursion { token_source, .. } => *token_source,
            AnalyzerError::ExceedLimit { token_source, .. } => *token_source,
            AnalyzerError::AmbiguousElsif { token_source, .. } => *token_source,
            AnalyzerError::LastItemWithDefine { token_source, .. } => *token_source,
            AnalyzerError::UnsignedLoopVariableInDescendingOrderForLoop {
                token_source, ..
            } => *token_source,
            AnalyzerError::FixedTypeWithSignedModifier { token_source, .. } => *token_source,
            AnalyzerError::UnsupportedByIr { token_source, .. } => *token_source,
        }
    }

    pub fn anonymous_identifier_usage(token: &TokenRange) -> Self {
        AnalyzerError::AnonymousIdentifierUsage {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn call_non_function(identifier: &str, kind: &str, token: &TokenRange) -> Self {
        AnalyzerError::CallNonFunction {
            identifier: identifier.to_string(),
            kind: kind.to_string(),
            input: source(token),
            error_location: token.into(),
            inst_context: vec![],
            token_source: token.source(),
        }
    }

    pub fn cyclic_type_dependency(start: &str, end: &str, token: &TokenRange) -> Self {
        AnalyzerError::CyclicTypeDependency {
            start: start.into(),
            end: end.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn duplicated_identifier(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::DuplicatedIdentifier {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn multiple_assignment(
        identifier: &str,
        token: &TokenRange,
        assigned: &[TokenRange],
    ) -> Self {
        AnalyzerError::MultipleAssignment {
            identifier: identifier.to_string(),
            input: source(token),
            error_locations: assigned.iter().map(|x| x.into()).collect(),
            token_source: token.source(),
        }
    }

    pub fn invalid_assignment(identifier: &str, kind: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidAssignment {
            identifier: identifier.into(),
            kind: kind.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_assignment_to_const(identifier: &str, kind: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidAssignmentToConst {
            identifier: identifier.into(),
            kind: kind.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_connect_operand(identifier: &str, reason: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidConnectOperand {
            identifier: identifier.into(),
            reason: reason.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_modifier(kind: &str, reason: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidModifier {
            kind: kind.to_string(),
            reason: reason.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_direction(kind: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidDirection {
            kind: kind.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_operand(kind: &str, op: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidOperand {
            kind: kind.to_string(),
            op: op.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_logical_operand(op: bool, token: &TokenRange) -> Self {
        let kind = if op {
            "Operand of logical operator"
        } else {
            "Conditional expression"
        };
        AnalyzerError::InvalidLogicalOperand {
            kind: kind.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_factor(
        identifier: Option<&str>,
        kind: &str,
        token: &TokenRange,
        inst_context: &[TokenRange],
    ) -> Self {
        let (input, inst_context) = source_with_context(token, inst_context);
        AnalyzerError::InvalidFactor {
            identifier: identifier.map(|x| x.to_string()),
            kind: kind.to_string(),
            input,
            error_location: token.into(),
            inst_context,
            token_source: token.source(),
        }
    }

    pub fn invalid_select(
        kind: &InvalidSelectKind,
        token: &TokenRange,
        inst_context: &[TokenRange],
    ) -> Self {
        let (input, inst_context) = source_with_context(token, inst_context);
        AnalyzerError::InvalidSelect {
            kind: kind.clone(),
            input,
            error_location: token.into(),
            inst_context,
            token_source: token.source(),
        }
    }

    pub fn invalid_identifier(identifier: &str, rule: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidIdentifier {
            identifier: identifier.to_string(),
            rule: rule.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_import(token: &TokenRange) -> Self {
        AnalyzerError::InvalidImport {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_lsb(token: &TokenRange) -> Self {
        AnalyzerError::InvalidLsb {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_msb(token: &TokenRange) -> Self {
        AnalyzerError::InvalidMsb {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_number_character(cause: char, kind: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidNumberCharacter {
            cause,
            kind: kind.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_statement(kind: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidStatement {
            kind: kind.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_clock(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidClock {
            identifier: identifier.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn multiple_default_clock(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::MultipleDefaultClock {
            identifier: identifier.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_reset(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidReset {
            identifier: identifier.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn multiple_default_reset(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::MultipleDefaultReset {
            identifier: identifier.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_reset_non_elaborative(token: &TokenRange) -> Self {
        AnalyzerError::InvalidResetNonElaborative {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_case_condition_non_elaborative(token: &TokenRange) -> Self {
        AnalyzerError::InvalidCaseConditionNonElaborative {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_const_non_elaborative(token: &TokenRange) -> Self {
        AnalyzerError::InvalidConstNonElaborative {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_cast(from: &str, to: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidCast {
            from: from.into(),
            to: to.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_test(cause: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidTest {
            cause: cause.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_type_declaration(kind: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidTypeDeclaration {
            kind: kind.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_modport_variable_item(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidModportVariableItem {
            identifier: identifier.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_modport_function_item(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidModportFunctionItem {
            identifier: identifier.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unexpandable_modport(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnexpandableModport {
            identifier: identifier.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_port_default_value(
        kind: InvalidPortDefaultValueKind,
        token: &TokenRange,
    ) -> Self {
        AnalyzerError::InvalidPortDefaultValue {
            kind,
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn incompat_proto(identifier: &str, proto: &str, cause: &str, token: &TokenRange) -> Self {
        AnalyzerError::IncompatProto {
            identifier: identifier.into(),
            proto: proto.into(),
            cause: cause.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn missing_default_argument(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::MissingDefaultArgument {
            identifier: identifier.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn mismatch_function_arity(
        name: &str,
        arity: usize,
        args: usize,
        token: &TokenRange,
    ) -> Self {
        AnalyzerError::MismatchFunctionArity {
            name: name.to_string(),
            arity,
            args,
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn mismatch_function_arg(name: &str, src: &str, token: &TokenRange) -> Self {
        AnalyzerError::MismatchFunctionArg {
            name: name.to_string(),
            src: src.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn mismatch_generics_arity(
        name: &str,
        arity: usize,
        args: usize,
        token: &TokenRange,
    ) -> Self {
        AnalyzerError::MismatchGenericsArity {
            name: name.to_string(),
            arity,
            args,
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn mismatch_type(name: &str, expected: &str, actual: &str, token: &TokenRange) -> Self {
        AnalyzerError::MismatchType {
            name: name.to_string(),
            expected: expected.to_string(),
            actual: actual.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn mismatch_clock_domain(
        clock_domain: &str,
        other_domain: &str,
        token: &TokenRange,
        other_token: &TokenRange,
    ) -> Self {
        AnalyzerError::MismatchClockDomain {
            clock_domain: clock_domain.to_string(),
            other_domain: other_domain.to_string(),
            input: source(token),
            error_location: token.into(),
            other_location: other_token.into(),
            token_source: token.source(),
        }
    }

    pub fn mismatch_assignment(
        src: &str,
        dst: &str,
        token: &TokenRange,
        inst_context: &[TokenRange],
    ) -> Self {
        let (input, inst_context) = source_with_context(token, inst_context);
        AnalyzerError::MismatchAssignment {
            src: src.to_string(),
            dst: dst.to_string(),
            input,
            error_location: token.into(),
            inst_context,
            token_source: token.source(),
        }
    }

    pub fn missing_clock_signal(token: &TokenRange) -> Self {
        AnalyzerError::MissingClockSignal {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn missing_if_reset(token: &TokenRange) -> Self {
        AnalyzerError::MissingIfReset {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn missing_reset_signal(token: &TokenRange) -> Self {
        AnalyzerError::MissingResetSignal {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn missing_reset_statement(name: &str, token: &TokenRange, tokens: &[TokenRange]) -> Self {
        AnalyzerError::MissingResetStatement {
            name: name.to_string(),
            input: source(token),
            error_locations: tokens.iter().map(|x| x.into()).collect(),
            token_source: token.source(),
        }
    }

    pub fn missing_tri(token: &TokenRange) -> Self {
        AnalyzerError::MissingTri {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn missing_clock_domain(token: &TokenRange) -> Self {
        AnalyzerError::MissingClockDomain {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn mismatch_attribute_args(name: &str, expected: String, token: &TokenRange) -> Self {
        AnalyzerError::MismatchAttributeArgs {
            name: name.to_string(),
            expected,
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn missing_port(name: &str, port: &str, token: &TokenRange) -> Self {
        AnalyzerError::MissingPort {
            name: name.to_string(),
            port: port.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn mixed_function_argument(token: &TokenRange) -> Self {
        AnalyzerError::MixedFunctionArgument {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn sv_keyword_usage(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::SvKeywordUsage {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn sv_with_implicit_reset(token: &TokenRange) -> Self {
        AnalyzerError::SvWithImplicitReset {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_clock_domain(token: &TokenRange) -> Self {
        AnalyzerError::InvalidClockDomain {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn too_large_enum_variant(
        identifier: &str,
        value: isize,
        width: usize,
        token: &TokenRange,
    ) -> Self {
        AnalyzerError::TooLargeEnumVariant {
            identifier: identifier.to_string(),
            value,
            width,
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unevaluatable_enum_variant(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnevaluatableEnumVariant {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_enum_variant_value(
        identifier: &str,
        encoding: &str,
        token: &TokenRange,
    ) -> Self {
        AnalyzerError::InvalidEnumVariant {
            identifier: identifier.to_string(),
            encoding: encoding.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn too_large_number(width: usize, token: &TokenRange) -> Self {
        AnalyzerError::TooLargeNumber {
            width,
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn too_much_enum_variant(
        identifier: &str,
        number: usize,
        width: usize,
        token: &TokenRange,
    ) -> Self {
        AnalyzerError::TooMuchEnumVariant {
            identifier: identifier.to_string(),
            number,
            width,
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invisible_identifier(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvisibleIndentifier {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn undefined_identifier(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::UndefinedIdentifier {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn referring_before_definition(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::ReferringBeforeDefinition {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unresolvable_generic_argument(
        identifier: &str,
        token: &TokenRange,
        definition_token: &TokenRange,
    ) -> Self {
        AnalyzerError::UnresolvableGenericArgument {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
            definition_location: definition_token.into(),
            token_source: token.source(),
        }
    }

    pub fn unknown_attribute(name: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnknownAttribute {
            name: name.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_embed(way: &str, lang: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidEmbed {
            way: way.to_string(),
            lang: lang.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn invalid_embed_identifier(token: &TokenRange) -> Self {
        AnalyzerError::InvalidEmbedIdentifier {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unknown_embed_lang(name: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnknownEmbedLang {
            name: name.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unknown_embed_way(name: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnknownEmbedWay {
            name: name.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unknown_include_way(name: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnknownIncludeWay {
            name: name.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unknown_member(name: &str, member: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnknownMember {
            name: name.to_string(),
            member: member.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unknown_unsafe(name: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnknownUnsafe {
            name: name.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn private_member(name: &str, token: &TokenRange) -> Self {
        AnalyzerError::PrivateMember {
            name: name.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn private_namespace(name: &str, token: &TokenRange) -> Self {
        AnalyzerError::PrivateNamespace {
            name: name.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unknown_msb(token: &TokenRange) -> Self {
        AnalyzerError::UnknownMsb {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unknown_port(name: &str, port: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnknownPort {
            name: name.to_string(),
            port: port.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unknown_param(name: &str, param: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnknownParam {
            name: name.to_string(),
            param: param.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unenclosed_inner_if_expression(token: &TokenRange) -> Self {
        AnalyzerError::UnenclosedInnerIfExpression {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unused_variable(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnusedVariable {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unused_return(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnusedReturn {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unassign_variable(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnassignVariable {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unassignable_output(token: &TokenRange) -> Self {
        AnalyzerError::UnassignableOutput {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn uncovered_branch(identifier: &str, token: &TokenRange, covered: &[TokenRange]) -> Self {
        AnalyzerError::UncoveredBranch {
            identifier: identifier.to_string(),
            input: source(token),
            error_locations: covered.iter().map(|x| x.into()).collect(),
            token_source: token.source(),
        }
    }

    pub fn reserved_identifier(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::ReservedIdentifier {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn include_failure(name: &str, cause: &str, token: &TokenRange) -> Self {
        AnalyzerError::IncludeFailure {
            name: name.to_string(),
            cause: cause.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn wrong_seperator(separator: &str, token: &TokenRange) -> Self {
        let valid_separator = if separator == "." { "::" } else { "." };
        AnalyzerError::WrongSeparator {
            separator: separator.to_string(),
            valid_separator: valid_separator.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn infinite_recursion(token: &TokenRange) -> Self {
        AnalyzerError::InfiniteRecursion {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn exceed_limit(kind: &str, value: usize, token: &TokenRange) -> Self {
        AnalyzerError::ExceedLimit {
            kind: kind.to_string(),
            value,
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn ambiguous_elsif(cause: &str, token: &TokenRange) -> Self {
        AnalyzerError::AmbiguousElsif {
            cause: cause.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn last_item_with_define(token: &TokenRange) -> Self {
        AnalyzerError::LastItemWithDefine {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unsigned_loop_variable_in_descending_order_for_loop(token: &TokenRange) -> Self {
        AnalyzerError::UnsignedLoopVariableInDescendingOrderForLoop {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn fixed_type_with_signed_modifier(token: &TokenRange) -> Self {
        AnalyzerError::FixedTypeWithSignedModifier {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unsupported_by_ir(code: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnsupportedByIr {
            code: code.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidSelectKind {
    WrongOrder { beg: usize, end: usize },
    OutOfRange { beg: usize, end: usize, size: usize },
    OutOfDimension { dim: usize, size: usize },
}

impl fmt::Display for InvalidSelectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidSelectKind::WrongOrder { beg, end } => {
                format!("wrong index order [{beg}:{end}]").fmt(f)
            }
            InvalidSelectKind::OutOfRange { beg, end, size } => {
                if beg == end {
                    format!("out of range [{beg}] > {size}").fmt(f)
                } else {
                    format!("out of range [{beg}:{end}] > {size}").fmt(f)
                }
            }
            InvalidSelectKind::OutOfDimension { .. } => "out of dimension".fmt(f),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidPortDefaultValueKind {
    NotGlobal,
    InFunction,
    NonAnonymousInOutput,
    InvalidDirection(String),
}

impl fmt::Display for InvalidPortDefaultValueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidPortDefaultValueKind::NotGlobal => {
                "port default value should be accessable globally".fmt(f)
            }
            InvalidPortDefaultValueKind::InFunction => {
                "port default value in function is not supported".fmt(f)
            }
            InvalidPortDefaultValueKind::NonAnonymousInOutput => {
                "Only '_' is supported for output default value".fmt(f)
            }
            InvalidPortDefaultValueKind::InvalidDirection(x) => {
                format!("Port default value for {x} is not supported").fmt(f)
            }
        }
    }
}

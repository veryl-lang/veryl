use crate::multi_sources::{MultiSources, Source};
use miette::{self, Diagnostic, Severity, SourceSpan};
use std::fmt;
use thiserror::Error;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::TokenSource;

#[derive(Error, Diagnostic, Debug, PartialEq, Eq)]
pub enum AnalyzerError {
    #[diagnostic(
        severity(Error),
        code(ambiguous_elsif),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(anonymous_identifier_usage),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("Cyclic dependency between \"{start}\" and \"{end}\"")]
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" is duplicated")]
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
        code(exceed_limit),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("exceed {kind} limit: {value}")]
    ExceedLimit {
        kind: ExceedLimitKind,
        value: usize,
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(include_failure),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(incompat_proto),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" is incompatible with \"{proto}\" because {cause}")]
    IncompatProto {
        identifier: String,
        proto: String,
        cause: IncompatProtoKind,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(infinite_recursion),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("infinite instance recursion is detected")]
    InfiniteRecursion {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_assignment),
        help("remove the assignment"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" can't be assigned because it is {kind}")]
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
        code(invalid_cast),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
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
        code(invalid_clock),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error(
        "\"{identifier}\" can't be used as a clock because it is not 'clock' type nor a single bit signal"
    )]
    InvalidClock {
        identifier: String,
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(invalid_connect_operand),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" can't be used as a connect operand because {reason}")]
    InvalidConnectOperand {
        identifier: String,
        reason: InvalidConnectOperandKind,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_direction),
        help("remove {kind} direction"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(invalid_embed),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
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

    #[diagnostic(
        severity(Error),
        code(invalid_embed_identifier),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("embed identifier can be used in (way: inline/lang: sv) code block only")]
    InvalidEmbedIdentifier {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_enum_variant),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("The value of enum variant \"{identifier}\" is not encoded value by {encoding}")]
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
        code(invalid_factor),
        help("remove {kind} from expression"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("{} cannot be used as a factor in an expression", match .identifier {
        Some(x) => format!("\"{}\" of {}", x, .kind),
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
        code(invalid_identifier),
        help("follow naming rule"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" violate \"{rule}\" naming rule")]
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        severity(Warning),
        code(invalid_logical_operand),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(invalid_lsb),
        help("remove lsb"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(invalid_modifier),
        help("remove the modifier"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("{kind} modifier can't be used because {reason}")]
    InvalidModifier {
        kind: String,
        reason: InvalidModifierKind,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_modport_item),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" is not a {kind}")]
    InvalidModportItem {
        kind: InvalidModportItemKind,
        identifier: String,
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(invalid_operand),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        severity(Error),
        code(invalid_port_default_value),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error(
        "\"{identifier}\" can't be used as a reset because it is not 'reset' type nor a single bit signal"
    )]
    InvalidReset {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Warning),
        code(invalid_select),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        severity(Error),
        code(invalid_statement),
        help("remove {kind} statement"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(invalid_test),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("test is invalid because {cause}")]
    InvalidTest {
        cause: InvalidTestKind,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_type_declaration),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("{kind} can't be declared in interface declaration")]
    InvalidTypeDeclaration {
        kind: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(invisible_identifier),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("cannot refer indentifier \"{identifier}\" because it is invisible at here")]
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
        code(last_item_with_define),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(mismatch_assignment),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(mismatch_attribute_args),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(mismatch_clock_domain),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(mismatch_function_arg),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(mismatch_function_arity),
        help("fix function arguments"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        severity(Error),
        code(mismatch_generics_arity),
        help("fix generics arguments"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(mismatch_type),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(missing_clock_domain),
        help("add clock domain annotation"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(missing_clock_signal),
        help("add clock port"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(missing_default_argument),
        help("give default argument"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("missing default argument for parameter \"{identifier}\"")]
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
        code(missing_if_reset),
        help("add if_reset statement"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(missing_reset_signal),
        help("add reset port"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{name}\" is not reset in if_reset statement")]
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(mixed_function_argument),
        help("fix function arguments"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        severity(Warning),
        code(mixed_struct_union_member),
        help("unify struct/union members to either 2-state member or 4-state member"),
        url("")
    )]
    #[error("2-state member and 4-state member are mixed in the same struct/union")]
    MixedStructUnionMember {
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" is assigned in multiple procedural blocks or assignment statements")]
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
        code(multiple_default),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error(
        "\"{identifier}\" can't be used as the default {kind} because the default {kind} has already been specified."
    )]
    MultipleDefault {
        kind: MultipleDefaultKind,
        identifier: String,
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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

    #[diagnostic(
        severity(Error),
        code(private_namespace),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
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
        code(referring_before_definition),
        help("move definition before reference point"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" is referred before it is defined.")]
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
        code(reserved_identifier),
        help("prefix `__` can't be used"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" is reverved for compiler usage")]
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
        code(sv_keyword_usage),
        help("Change the identifier to a non-SystemVerilog keyword"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(too_large_enum_variant),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error(
        "The value of enum variant \"{identifier}\" is {value}, it is can't be represented by {width} bits"
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
        code(too_large_number),
        help("increase bit width"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error(
        "enum \"{identifier}\" has {number} variants, they are can't be represented by {width} bits"
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

    #[diagnostic(
        severity(Warning),
        code(unassign_variable),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" is unassigned")]
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" is not covered by all branches, it causes latch generation")]
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
        code(undefined_identifier),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" is undefined")]
    UndefinedIdentifier {
        identifier: String,
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        severity(Error),
        code(unevaluatable_value),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("Value, which is not evaluable at elaboration, can't be used as {kind}")]
    UnevaluableValue {
        kind: UnevaluableValueKind,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unexpandable_modport),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" can't be expanded")]
    UnexpandableModport {
        identifier: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unknown_attribute),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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

    #[diagnostic(
        severity(Error),
        code(unknown_embed_lang),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(unknown_msb),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(unknown_param),
        help("remove \"{param}\" param"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        severity(Error),
        code(unknown_port),
        help("remove \"{port}\" port"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(unknown_unsafe),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(unresolvable_generic_argument),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" can't be resolved from the definition of generics")]
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
        severity(Warning),
        code(unsigned_loop_variable_in_descending_order_for_loop),
        help("use singed type as loop variable"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
        code(unsupported_by_ir),
        help(""),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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

    #[diagnostic(
        severity(Warning),
        code(unused_return),
        help("add variable assignment for function return"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("return value of \"{identifier}\" is unused")]
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
        code(unused_variable),
        help("add prefix `_` to unused variable name"),
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
    )]
    #[error("\"{identifier}\" is unused")]
    UnusedVariable {
        identifier: String,
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
        url("https://doc.veryl-lang.org/book/07_appendix/02_semantic_error.html#{}", self.code().unwrap())
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
            AnalyzerError::AmbiguousElsif { token_source, .. } => *token_source,
            AnalyzerError::AnonymousIdentifierUsage { token_source, .. } => *token_source,
            AnalyzerError::CallNonFunction { token_source, .. } => *token_source,
            AnalyzerError::CyclicTypeDependency { token_source, .. } => *token_source,
            AnalyzerError::DuplicatedIdentifier { token_source, .. } => *token_source,
            AnalyzerError::ExceedLimit { token_source, .. } => *token_source,
            AnalyzerError::FixedTypeWithSignedModifier { token_source, .. } => *token_source,
            AnalyzerError::IncludeFailure { token_source, .. } => *token_source,
            AnalyzerError::IncompatProto { token_source, .. } => *token_source,
            AnalyzerError::InfiniteRecursion { token_source, .. } => *token_source,
            AnalyzerError::InvalidAssignment { token_source, .. } => *token_source,
            AnalyzerError::InvalidCast { token_source, .. } => *token_source,
            AnalyzerError::InvalidClock { token_source, .. } => *token_source,
            AnalyzerError::InvalidClockDomain { token_source, .. } => *token_source,
            AnalyzerError::InvalidConnectOperand { token_source, .. } => *token_source,
            AnalyzerError::InvalidDirection { token_source, .. } => *token_source,
            AnalyzerError::InvalidEmbed { token_source, .. } => *token_source,
            AnalyzerError::InvalidEmbedIdentifier { token_source, .. } => *token_source,
            AnalyzerError::InvalidEnumVariant { token_source, .. } => *token_source,
            AnalyzerError::InvalidFactor { token_source, .. } => *token_source,
            AnalyzerError::InvalidIdentifier { token_source, .. } => *token_source,
            AnalyzerError::InvalidImport { token_source, .. } => *token_source,
            AnalyzerError::InvalidLogicalOperand { token_source, .. } => *token_source,
            AnalyzerError::InvalidLsb { token_source, .. } => *token_source,
            AnalyzerError::InvalidModifier { token_source, .. } => *token_source,
            AnalyzerError::InvalidModportItem { token_source, .. } => *token_source,
            AnalyzerError::InvalidMsb { token_source, .. } => *token_source,
            AnalyzerError::InvalidNumberCharacter { token_source, .. } => *token_source,
            AnalyzerError::InvalidOperand { token_source, .. } => *token_source,
            AnalyzerError::InvalidPortDefaultValue { token_source, .. } => *token_source,
            AnalyzerError::InvalidReset { token_source, .. } => *token_source,
            AnalyzerError::InvalidSelect { token_source, .. } => *token_source,
            AnalyzerError::InvalidStatement { token_source, .. } => *token_source,
            AnalyzerError::InvalidTest { token_source, .. } => *token_source,
            AnalyzerError::InvalidTypeDeclaration { token_source, .. } => *token_source,
            AnalyzerError::InvisibleIndentifier { token_source, .. } => *token_source,
            AnalyzerError::LastItemWithDefine { token_source, .. } => *token_source,
            AnalyzerError::MismatchAssignment { token_source, .. } => *token_source,
            AnalyzerError::MismatchAttributeArgs { token_source, .. } => *token_source,
            AnalyzerError::MismatchClockDomain { token_source, .. } => *token_source,
            AnalyzerError::MismatchFunctionArg { token_source, .. } => *token_source,
            AnalyzerError::MismatchFunctionArity { token_source, .. } => *token_source,
            AnalyzerError::MismatchGenericsArity { token_source, .. } => *token_source,
            AnalyzerError::MismatchType { token_source, .. } => *token_source,
            AnalyzerError::MissingClockDomain { token_source, .. } => *token_source,
            AnalyzerError::MissingClockSignal { token_source, .. } => *token_source,
            AnalyzerError::MissingDefaultArgument { token_source, .. } => *token_source,
            AnalyzerError::MissingIfReset { token_source, .. } => *token_source,
            AnalyzerError::MissingPort { token_source, .. } => *token_source,
            AnalyzerError::MissingResetSignal { token_source, .. } => *token_source,
            AnalyzerError::MissingResetStatement { token_source, .. } => *token_source,
            AnalyzerError::MissingTri { token_source, .. } => *token_source,
            AnalyzerError::MixedFunctionArgument { token_source, .. } => *token_source,
            AnalyzerError::MixedStructUnionMember { token_source, .. } => *token_source,
            AnalyzerError::MultipleAssignment { token_source, .. } => *token_source,
            AnalyzerError::MultipleDefault { token_source, .. } => *token_source,
            AnalyzerError::PrivateMember { token_source, .. } => *token_source,
            AnalyzerError::PrivateNamespace { token_source, .. } => *token_source,
            AnalyzerError::ReferringBeforeDefinition { token_source, .. } => *token_source,
            AnalyzerError::ReservedIdentifier { token_source, .. } => *token_source,
            AnalyzerError::SvKeywordUsage { token_source, .. } => *token_source,
            AnalyzerError::SvWithImplicitReset { token_source, .. } => *token_source,
            AnalyzerError::TooLargeEnumVariant { token_source, .. } => *token_source,
            AnalyzerError::TooLargeNumber { token_source, .. } => *token_source,
            AnalyzerError::TooMuchEnumVariant { token_source, .. } => *token_source,
            AnalyzerError::UnassignVariable { token_source, .. } => *token_source,
            AnalyzerError::UnassignableOutput { token_source, .. } => *token_source,
            AnalyzerError::UncoveredBranch { token_source, .. } => *token_source,
            AnalyzerError::UndefinedIdentifier { token_source, .. } => *token_source,
            AnalyzerError::UnenclosedInnerIfExpression { token_source, .. } => *token_source,
            AnalyzerError::UnevaluableValue { token_source, .. } => *token_source,
            AnalyzerError::UnexpandableModport { token_source, .. } => *token_source,
            AnalyzerError::UnknownAttribute { token_source, .. } => *token_source,
            AnalyzerError::UnknownEmbedLang { token_source, .. } => *token_source,
            AnalyzerError::UnknownEmbedWay { token_source, .. } => *token_source,
            AnalyzerError::UnknownIncludeWay { token_source, .. } => *token_source,
            AnalyzerError::UnknownMember { token_source, .. } => *token_source,
            AnalyzerError::UnknownMsb { token_source, .. } => *token_source,
            AnalyzerError::UnknownParam { token_source, .. } => *token_source,
            AnalyzerError::UnknownPort { token_source, .. } => *token_source,
            AnalyzerError::UnknownUnsafe { token_source, .. } => *token_source,
            AnalyzerError::UnresolvableGenericArgument { token_source, .. } => *token_source,
            AnalyzerError::UnsignedLoopVariableInDescendingOrderForLoop {
                token_source, ..
            } => *token_source,
            AnalyzerError::UnsupportedByIr { token_source, .. } => *token_source,
            AnalyzerError::UnusedReturn { token_source, .. } => *token_source,
            AnalyzerError::UnusedVariable { token_source, .. } => *token_source,
            AnalyzerError::WrongSeparator { token_source, .. } => *token_source,
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
    pub fn exceed_limit(kind: ExceedLimitKind, value: usize, token: &TokenRange) -> Self {
        AnalyzerError::ExceedLimit {
            kind,
            value,
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
    pub fn include_failure(name: &str, cause: &str, token: &TokenRange) -> Self {
        AnalyzerError::IncludeFailure {
            name: name.to_string(),
            cause: cause.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }
    pub fn incompat_proto(
        identifier: &str,
        proto: &str,
        cause: IncompatProtoKind,
        token: &TokenRange,
    ) -> Self {
        AnalyzerError::IncompatProto {
            identifier: identifier.into(),
            proto: proto.into(),
            cause,
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
    pub fn invalid_assignment(identifier: &str, kind: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidAssignment {
            identifier: identifier.into(),
            kind: kind.into(),
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
    pub fn invalid_clock(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidClock {
            identifier: identifier.into(),
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
    pub fn invalid_connect_operand(
        identifier: &str,
        reason: InvalidConnectOperandKind,
        token: &TokenRange,
    ) -> Self {
        AnalyzerError::InvalidConnectOperand {
            identifier: identifier.into(),
            reason,
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
    pub fn invalid_enum_variant(identifier: &str, encoding: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidEnumVariant {
            identifier: identifier.to_string(),
            encoding: encoding.to_string(),
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
    pub fn invalid_lsb(token: &TokenRange) -> Self {
        AnalyzerError::InvalidLsb {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }
    pub fn invalid_modifier(kind: &str, reason: InvalidModifierKind, token: &TokenRange) -> Self {
        AnalyzerError::InvalidModifier {
            kind: kind.to_string(),
            reason,
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }
    pub fn invalid_modport_item(
        kind: InvalidModportItemKind,
        identifier: &str,
        token: &TokenRange,
    ) -> Self {
        AnalyzerError::InvalidModportItem {
            kind,
            identifier: identifier.into(),
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
    pub fn invalid_operand(kind: &str, op: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidOperand {
            kind: kind.to_string(),
            op: op.to_string(),
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
    pub fn invalid_reset(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidReset {
            identifier: identifier.into(),
            input: source(token),
            error_location: token.into(),
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
    pub fn invalid_statement(kind: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvalidStatement {
            kind: kind.to_string(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }
    pub fn invalid_test(cause: InvalidTestKind, token: &TokenRange) -> Self {
        AnalyzerError::InvalidTest {
            cause,
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
    pub fn invisible_identifier(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::InvisibleIndentifier {
            identifier: identifier.to_string(),
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
    pub fn mismatch_attribute_args(name: &str, expected: String, token: &TokenRange) -> Self {
        AnalyzerError::MismatchAttributeArgs {
            name: name.to_string(),
            expected,
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
    pub fn mismatch_function_arg(name: &str, src: &str, token: &TokenRange) -> Self {
        AnalyzerError::MismatchFunctionArg {
            name: name.to_string(),
            src: src.to_string(),
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
    pub fn missing_clock_domain(token: &TokenRange) -> Self {
        AnalyzerError::MissingClockDomain {
            input: source(token),
            error_location: token.into(),
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
    pub fn missing_default_argument(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::MissingDefaultArgument {
            identifier: identifier.into(),
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
    pub fn missing_port(name: &str, port: &str, token: &TokenRange) -> Self {
        AnalyzerError::MissingPort {
            name: name.to_string(),
            port: port.to_string(),
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
    pub fn mixed_function_argument(token: &TokenRange) -> Self {
        AnalyzerError::MixedFunctionArgument {
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }
    pub fn mixed_struct_union_member(token: &TokenRange) -> Self {
        AnalyzerError::MixedStructUnionMember {
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
    pub fn multiple_default(
        kind: MultipleDefaultKind,
        identifier: &str,
        token: &TokenRange,
    ) -> Self {
        AnalyzerError::MultipleDefault {
            kind,
            identifier: identifier.into(),
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
    pub fn referring_before_definition(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::ReferringBeforeDefinition {
            identifier: identifier.to_string(),
            input: source(token),
            error_location: token.into(),
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
    pub fn undefined_identifier(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::UndefinedIdentifier {
            identifier: identifier.to_string(),
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
    pub fn unevaluable_value(kind: UnevaluableValueKind, token: &TokenRange) -> Self {
        AnalyzerError::UnevaluableValue {
            kind,
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
    pub fn unknown_attribute(name: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnknownAttribute {
            name: name.to_string(),
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
    pub fn unknown_msb(token: &TokenRange) -> Self {
        AnalyzerError::UnknownMsb {
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
    pub fn unknown_port(name: &str, port: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnknownPort {
            name: name.to_string(),
            port: port.to_string(),
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
    pub fn unsigned_loop_variable_in_descending_order_for_loop(token: &TokenRange) -> Self {
        AnalyzerError::UnsignedLoopVariableInDescendingOrderForLoop {
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
    pub fn unused_return(identifier: &str, token: &TokenRange) -> Self {
        AnalyzerError::UnusedReturn {
            identifier: identifier.to_string(),
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
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExceedLimitKind {
    EvaluateSize,
    HierarchyDepth,
    TotalInstance,
}

impl fmt::Display for ExceedLimitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExceedLimitKind::EvaluateSize => "evaluate size".fmt(f),
            ExceedLimitKind::HierarchyDepth => "hierarchy depth".fmt(f),
            ExceedLimitKind::TotalInstance => "total instance".fmt(f),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IncompatProtoKind {
    IncompatibleAlias(StrId),
    IncompatibleFunction(StrId),
    IncompatibleGenericParam(StrId),
    IncompatibleMember(StrId),
    IncompatibleModport(StrId),
    IncompatibleParam(StrId),
    IncompatiblePort(StrId),
    IncompatibleType,
    IncompatibleTypedef(StrId),
    IncompatibleVar(StrId),
    MissignMember(StrId),
    MissingAlias(StrId),
    MissingFunction(StrId),
    MissingGenericParam(StrId),
    MissingModport(StrId),
    MissingParam(StrId),
    MissingPort(StrId),
    MissingType,
    MissingTypedef(StrId),
    MissingVar(StrId),
    UnnecessaryGenericParam(StrId),
    UnnecessaryMember(StrId),
    UnnecessaryParam(StrId),
    UnnecessaryPort(StrId),
    UnnecessaryType,
}

impl fmt::Display for IncompatProtoKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IncompatProtoKind::IncompatibleAlias(x) => {
                format!("alias \"{x}\" is incompatible").fmt(f)
            }
            IncompatProtoKind::IncompatibleFunction(x) => {
                format!("function \"{x}\" is incompatible").fmt(f)
            }
            IncompatProtoKind::IncompatibleGenericParam(x) => {
                format!("generic parameter \"{x}\" is incompatible").fmt(f)
            }
            IncompatProtoKind::IncompatibleMember(x) => {
                format!("member \"{x}\" is incompatible").fmt(f)
            }
            IncompatProtoKind::IncompatibleModport(x) => {
                format!("modport \"{x}\" is incompatible").fmt(f)
            }
            IncompatProtoKind::IncompatibleParam(x) => {
                format!("parameter \"{x}\" has incompatible type").fmt(f)
            }
            IncompatProtoKind::IncompatiblePort(x) => {
                format!("port \"{x}\" has incompatible type").fmt(f)
            }
            IncompatProtoKind::IncompatibleType => "type specification is incompatible".fmt(f),
            IncompatProtoKind::IncompatibleTypedef(x) => {
                format!("type definition \"{x}\" is incompatible").fmt(f)
            }
            IncompatProtoKind::IncompatibleVar(x) => {
                format!("variable \"{x}\" is incompatible").fmt(f)
            }
            IncompatProtoKind::MissignMember(x) => format!("member \"{x}\" is missing").fmt(f),
            IncompatProtoKind::MissingAlias(x) => format!("alias \"{x}\" is missing").fmt(f),
            IncompatProtoKind::MissingFunction(x) => format!("function \"{x}\" is missing").fmt(f),
            IncompatProtoKind::MissingGenericParam(x) => {
                format!("generic parameter \"{x}\" is missing").fmt(f)
            }
            IncompatProtoKind::MissingModport(x) => format!("modport \"{x}\" is missing").fmt(f),
            IncompatProtoKind::MissingParam(x) => format!("parameter \"{x}\" is missing").fmt(f),
            IncompatProtoKind::MissingPort(x) => format!("port \"{x}\" is missing").fmt(f),
            IncompatProtoKind::MissingType => "type specification is missing".fmt(f),
            IncompatProtoKind::MissingTypedef(x) => {
                format!("type definition \"{x}\" is missing").fmt(f)
            }
            IncompatProtoKind::MissingVar(x) => format!("variable \"{x}\" is missing").fmt(f),
            IncompatProtoKind::UnnecessaryGenericParam(x) => {
                format!("generic parameter \"{x}\" is unnecessary").fmt(f)
            }
            IncompatProtoKind::UnnecessaryMember(x) => {
                format!("member \"{x}\" is unnecessary").fmt(f)
            }
            IncompatProtoKind::UnnecessaryParam(x) => {
                format!("parameter \"{x}\" is unnecessary").fmt(f)
            }
            IncompatProtoKind::UnnecessaryPort(x) => format!("port \"{x}\" is unnecessary").fmt(f),
            IncompatProtoKind::UnnecessaryType => "type specification is unnecessary".fmt(f),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InvalidConnectOperandKind {
    IncludeInout,
    InstanceArray,
    ModportArray,
    UnemittableCast,
}

impl fmt::Display for InvalidConnectOperandKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidConnectOperandKind::IncludeInout => {
                "modport including inout ports can't be used at here".fmt(f)
            }
            InvalidConnectOperandKind::InstanceArray => "it is an array interface instance".fmt(f),
            InvalidConnectOperandKind::ModportArray => "it is an array modport".fmt(f),
            InvalidConnectOperandKind::UnemittableCast => "modport including variables of which type is defined in the interface can't be used for a connect operand".fmt(f),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InvalidModifierKind {
    NotTopModule,
    NotClockReset,
}

impl fmt::Display for InvalidModifierKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidModifierKind::NotTopModule => "here is not the module top layer".fmt(f),
            InvalidModifierKind::NotClockReset => {
                "the given type is not a single bit clock nor a single bit reset".fmt(f)
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InvalidModportItemKind {
    Function,
    Variable,
}

impl fmt::Display for InvalidModportItemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidModportItemKind::Function => "function".fmt(f),
            InvalidModportItemKind::Variable => "variable".fmt(f),
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InvalidTestKind {
    NoTopModuleCocotb,
}

impl fmt::Display for InvalidTestKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidTestKind::NoTopModuleCocotb => "`cocotb` test requires top module name at the second argument of `#[test]` attribute".fmt(f),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MultipleDefaultKind {
    Clock,
    Reset,
}

impl fmt::Display for MultipleDefaultKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MultipleDefaultKind::Clock => "clock".fmt(f),
            MultipleDefaultKind::Reset => "reset".fmt(f),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UnevaluableValueKind {
    CaseCondition,
    ConstValue,
    EnumVariant,
    ResetValue,
}

impl fmt::Display for UnevaluableValueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnevaluableValueKind::CaseCondition => "case condition".fmt(f),
            UnevaluableValueKind::ConstValue => "const value".fmt(f),
            UnevaluableValueKind::EnumVariant => "enum variant".fmt(f),
            UnevaluableValueKind::ResetValue => "reset value".fmt(f),
        }
    }
}

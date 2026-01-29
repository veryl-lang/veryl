use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::ir::Comptime;
use crate::r#unsafe::Unsafe;
use crate::unsafe_table;
use veryl_parser::token_range::TokenRange;

pub fn check_clock_domain(
    context: &mut Context,
    lhs: &Comptime,
    rhs: &Comptime,
    token: &TokenRange,
) {
    let cdc_unsafe = unsafe_table::contains(token, Unsafe::Cdc);
    if !lhs.clock_domain.compatible(&rhs.clock_domain) && !cdc_unsafe {
        context.insert_error(AnalyzerError::mismatch_clock_domain(
            &lhs.clock_domain.to_string(),
            &rhs.clock_domain.to_string(),
            &lhs.token,
            &rhs.token,
        ));
    }
}

//! The worksheet function library.

mod criteria;
mod date;
mod finance;
mod info;
mod logic;
mod lookup;
mod math;
mod stats;
mod text;

pub use criteria::{wildcard_match, Criteria};

use super::ast::Expr;
use super::eval::{Eval, Evaluator, SheetArea};
use crate::value::{CellError, Value};

/// Call a worksheet function. None when the function is unknown.
pub fn call(ev: &mut Evaluator, name: &str, args: &[Expr]) -> Option<Eval> {
    if let Some(result) = logic::call(ev, name, args) {
        return Some(result);
    }
    if let Some(result) = math::call(ev, name, args) {
        return Some(result);
    }
    if let Some(result) = stats::call(ev, name, args) {
        return Some(result);
    }
    if let Some(result) = lookup::call(ev, name, args) {
        return Some(result);
    }
    if let Some(result) = text::call(ev, name, args) {
        return Some(result);
    }
    if let Some(result) = date::call(ev, name, args) {
        return Some(result);
    }
    if let Some(result) = info::call(ev, name, args) {
        return Some(result);
    }
    finance::call(ev, name, args)
}

/// Whether the engine knows a function (for showing unsupported formulas).
pub fn is_known(name: &str) -> bool {
    KNOWN.contains(&name)
}

/// Every function name the library implements, for autocomplete.
pub fn names() -> &'static [&'static str] {
    KNOWN
}

pub(crate) const KNOWN: &[&str] = &[
    // logic
    "IF", "IFS", "IFERROR", "IFNA", "AND", "OR", "XOR", "NOT", "TRUE", "FALSE", "SWITCH",
    // math
    "SUM", "SUMIF", "SUMIFS", "SUMPRODUCT", "SUMSQ", "PRODUCT", "ABS", "SIGN", "INT", "TRUNC", "ROUND",
    "ROUNDUP", "ROUNDDOWN", "MROUND", "CEILING", "CEILING.MATH", "CEILING.PRECISE", "ISO.CEILING", "FLOOR",
    "FLOOR.MATH", "FLOOR.PRECISE", "EVEN", "ODD", "MOD", "QUOTIENT", "POWER", "SQRT", "SQRTPI", "EXP", "LN", "LOG",
    "LOG10", "PI", "SIN", "COS", "TAN", "ASIN", "ACOS", "ATAN", "ATAN2", "SINH", "COSH", "TANH", "ASINH", "ACOSH",
    "ATANH", "COT", "CSC", "SEC", "DEGREES", "RADIANS", "FACT", "FACTDOUBLE", "COMBIN", "COMBINA", "PERMUT",
    "GCD", "LCM", "RAND", "RANDBETWEEN", "SUBTOTAL", "AGGREGATE", "SUMX2MY2", "SUMX2PY2", "SUMXMY2",
    // statistics
    "COUNT", "COUNTA", "COUNTBLANK", "COUNTIF", "COUNTIFS", "AVERAGE", "AVERAGEA", "AVERAGEIF", "AVERAGEIFS",
    "MAX", "MAXA", "MAXIFS", "MIN", "MINA", "MINIFS", "MEDIAN", "MODE", "MODE.SNGL", "LARGE", "SMALL", "RANK",
    "RANK.EQ", "RANK.AVG", "PERCENTILE", "PERCENTILE.INC", "PERCENTILE.EXC", "QUARTILE", "QUARTILE.INC",
    "QUARTILE.EXC", "STDEV", "STDEV.S", "STDEVA", "STDEVP", "STDEV.P", "STDEVPA", "VAR", "VAR.S", "VARA", "VARP",
    "VAR.P", "VARPA", "AVEDEV", "DEVSQ", "GEOMEAN", "HARMEAN", "CORREL", "PEARSON", "COVAR", "COVARIANCE.P",
    "COVARIANCE.S", "SLOPE", "INTERCEPT", "RSQ", "FORECAST", "FORECAST.LINEAR", "PERCENTRANK", "PERCENTRANK.INC",
    // lookup
    "VLOOKUP", "HLOOKUP", "LOOKUP", "MATCH", "XMATCH", "XLOOKUP", "INDEX", "CHOOSE", "ROW", "ROWS", "COLUMN",
    "COLUMNS", "OFFSET", "INDIRECT", "ADDRESS", "TRANSPOSE", "HYPERLINK", "AREAS",
    // text
    "LEN", "LEFT", "RIGHT", "MID", "LOWER", "UPPER", "PROPER", "TRIM", "CLEAN", "CONCAT", "CONCATENATE",
    "TEXTJOIN", "REPT", "EXACT", "FIND", "SEARCH", "SUBSTITUTE", "REPLACE", "CHAR", "CODE", "UNICHAR",
    "UNICODE", "VALUE", "NUMBERVALUE", "TEXT", "FIXED", "DOLLAR", "T", "TEXTBEFORE", "TEXTAFTER",
    // dates
    "TODAY", "NOW", "DATE", "TIME", "YEAR", "MONTH", "DAY", "HOUR", "MINUTE", "SECOND", "WEEKDAY", "WEEKNUM",
    "ISOWEEKNUM", "EDATE", "EOMONTH", "DATEDIF", "DAYS", "DAYS360", "NETWORKDAYS", "NETWORKDAYS.INTL",
    "WORKDAY", "WORKDAY.INTL", "YEARFRAC", "DATEVALUE", "TIMEVALUE",
    // information
    "ISBLANK", "ISERR", "ISERROR", "ISNA", "ISLOGICAL", "ISNUMBER", "ISTEXT", "ISNONTEXT", "ISEVEN", "ISODD",
    "ISREF", "ISFORMULA", "FORMULATEXT", "N", "NA", "TYPE", "ERROR.TYPE",
    // finance
    "PMT", "IPMT", "PPMT", "FV", "PV", "NPER", "RATE", "NPV", "XNPV", "IRR", "XIRR", "MIRR", "SLN", "SYD", "DB",
    "DDB", "EFFECT", "NOMINAL", "CUMIPMT", "CUMPRINC", "PDURATION", "RRI",
];

/// Check an argument count; None when it is acceptable.
pub(crate) fn arity(args: &[Expr], min: usize, max: usize) -> Option<Eval> {
    if args.len() < min || args.len() > max {
        Some(Eval::error(CellError::Value))
    } else {
        None
    }
}

/// Unwrap a `Result<T, CellError>` inside a function returning `Eval`.
macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(err) => return $crate::formula::eval::Eval::error(err),
        }
    };
}
pub(crate) use tri;

/// A reference argument as a single area, or the error to return.
pub(crate) fn single_area(ev: &mut Evaluator, args: &[Expr], i: usize) -> Result<SheetArea, CellError> {
    match ev.reference_arg(args, i) {
        Some(areas) if areas.len() == 1 => Ok(areas[0]),
        Some(_) => Err(CellError::Value),
        None => Err(CellError::Value),
    }
}

/// The values of an argument as a 2-D array (references and arrays alike).
pub(crate) fn array_arg(ev: &mut Evaluator, args: &[Expr], i: usize) -> super::eval::Array {
    match args.get(i) {
        Some(e) => {
            let v = ev.eval(e);
            ev.to_array(v)
        }
        None => super::eval::Array::new(1, 1, vec![Value::Empty]),
    }
}

#[cfg(test)]
mod tests;

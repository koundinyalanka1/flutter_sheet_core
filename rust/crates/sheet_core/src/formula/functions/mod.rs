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
use super::eval::{Eval, Evaluator};
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

/// `tri!` for the dispatchers, which return `Option<Eval>`.
macro_rules! tri_some {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(err) => return Some($crate::formula::eval::Eval::error(err)),
        }
    };
}
pub(crate) use tri_some;


/// Evaluate every argument and apply `f` to their values. When any argument
/// is a multi-cell reference or an array, `f` runs element by element with
/// broadcasting (dynamic-array semantics) and the result is an array.
pub(crate) fn lift(ev: &mut Evaluator, args: &[Expr], f: &mut dyn FnMut(&[Value]) -> Value) -> Eval {
    let evals: Vec<Eval> = args.iter().map(|a| ev.eval(a)).collect();
    if !evals.iter().any(|e| ev.is_multi(e)) {
        let values: Vec<Value> = evals.into_iter().map(|e| ev.scalar(e)).collect();
        return Eval::Value(f(&values));
    }
    let arrays: Vec<super::eval::Array> = evals.into_iter().map(|e| ev.to_array(e)).collect();
    let rows = arrays.iter().map(|a| a.rows).max().unwrap_or(1);
    let cols = arrays.iter().map(|a| a.cols).max().unwrap_or(1);
    let mut data = Vec::with_capacity(rows * cols);
    let mut values = Vec::with_capacity(arrays.len());
    for r in 0..rows {
        for c in 0..cols {
            values.clear();
            values.extend(arrays.iter().map(|a| a.broadcast(r, c)));
            data.push(f(&values));
        }
    }
    Eval::Array(super::eval::Array::new(rows, cols, data))
}

/// A one-number function applied element by element.
pub(crate) fn lift_num1(ev: &mut Evaluator, args: &[Expr], f: impl Fn(f64) -> Result<f64, CellError>) -> Eval {
    if let Some(e) = arity(args, 1, 1) {
        return e;
    }
    lift(ev, args, &mut |v| match v[0].to_number().and_then(&f) {
        Ok(n) if n.is_finite() => Value::Number(n),
        Ok(_) => Value::Error(CellError::Num),
        Err(e) => Value::Error(e),
    })
}

/// A function of several numbers applied element by element. Omitted
/// trailing arguments take their defaults.
pub(crate) fn lift_nums(
    ev: &mut Evaluator,
    args: &[Expr],
    min: usize,
    defaults: &[f64],
    f: impl Fn(&[f64]) -> Result<f64, CellError>,
) -> Eval {
    let max = min + defaults.len();
    if let Some(e) = arity(args, min, max) {
        return e;
    }
    let given = args.len();
    let mut nums = vec![0.0; max];
    lift(ev, args, &mut |v| {
        for (i, x) in v.iter().enumerate() {
            match x.to_number() {
                Ok(n) => nums[i] = n,
                Err(e) => return Value::Error(e),
            }
        }
        for i in given..max {
            nums[i] = defaults[i - min];
        }
        match f(&nums) {
            Ok(n) if n.is_finite() => Value::Number(n),
            Ok(_) => Value::Error(CellError::Num),
            Err(e) => Value::Error(e),
        }
    })
}

/// Whether an optional argument was left out (absent or empty).
pub(crate) fn omitted(args: &[Expr], i: usize) -> bool {
    matches!(args.get(i), None | Some(Expr::Missing))
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

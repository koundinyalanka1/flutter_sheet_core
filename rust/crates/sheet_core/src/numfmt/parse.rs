//! Parsing format codes into sections of tokens.

use std::sync::LazyLock;

/// How AM/PM is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmPm {
    Upper,
    Lower,
    ShortUpper,
    ShortLower,
}

/// One element of a format section.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    /// Text written as is: quoted strings, escapes, and literal characters
    /// (including `%`, which also scales the value).
    Literal(String),
    /// `0`: a digit, or 0.
    Zero,
    /// `#`: a digit, or nothing.
    Hash,
    /// `?`: a digit, or a space.
    Question,
    /// The decimal point.
    Point,
    /// `E+`/`E-` (scientific notation).
    Exp,
    /// An unescaped comma while parsing; resolved into a thousands
    /// separator, a scaling comma or a literal before parsing finishes.
    Comma,
    /// The `/` of a fraction.
    Slash,
    /// `@`: the text value.
    At,
    /// `_x`: a space as wide as `x`.
    Space(char),
    /// `*x`: repeat `x` to fill the cell (written as nothing here).
    Fill(char),
    General,
    Year(u8),
    Month(u8),
    Day(u8),
    Hour(u8),
    Minute(u8),
    Second(u8),
    /// Fractional seconds: `.0`, `.00` or `.000` after seconds.
    SubSecond(u8),
    ElapsedHours(u8),
    ElapsedMinutes(u8),
    ElapsedSeconds(u8),
    AmPm(AmPm),
}

impl Token {
    pub fn is_digit(&self) -> bool {
        matches!(self, Token::Zero | Token::Hash | Token::Question)
    }

    fn is_date(&self) -> bool {
        matches!(
            self,
            Token::Year(_)
                | Token::Month(_)
                | Token::Day(_)
                | Token::Hour(_)
                | Token::Minute(_)
                | Token::Second(_)
                | Token::ElapsedHours(_)
                | Token::ElapsedMinutes(_)
                | Token::ElapsedSeconds(_)
                | Token::AmPm(_)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CmpOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

/// A bracketed condition such as `[>=100000]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Condition {
    pub op: CmpOp,
    pub value: f64,
}

impl Condition {
    pub fn matches(self, n: f64) -> bool {
        match self.op {
            CmpOp::Lt => n < self.value,
            CmpOp::Le => n <= self.value,
            CmpOp::Gt => n > self.value,
            CmpOp::Ge => n >= self.value,
            CmpOp::Eq => n == self.value,
            CmpOp::Ne => n != self.value,
        }
    }

    /// A condition that selects negative numbers on purpose (`[<0]`), whose
    /// section then shows them without an automatic minus sign.
    pub fn op_is_negative(self) -> bool {
        matches!(self.op, CmpOp::Lt | CmpOp::Le) && self.value <= 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExpSpec {
    pub upper: bool,
    /// `E+` writes a plus sign for positive exponents; `E-` does not.
    pub plus: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FractionSpec {
    /// Placeholders in the denominator (the largest denominator has this
    /// many digits).
    pub denominator_digits: usize,
    /// `?/8`: always eighths.
    pub fixed_denominator: Option<u64>,
}

/// One `;`-separated section of a format code.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub tokens: Vec<Token>,
    /// `0xRRGGBB`.
    pub color: Option<u32>,
    pub condition: Option<Condition>,
    pub is_date: bool,
    /// Number of `%` signs (each multiplies by 100).
    pub percent: usize,
    /// Number of scaling commas (each divides by 1000).
    pub scale: usize,
    /// A thousands separator between integer placeholders.
    pub thousands: bool,
    pub exponent: Option<ExpSpec>,
    pub fraction: Option<FractionSpec>,
}

impl Section {
    fn new(tokens: Vec<Token>) -> Self {
        Section {
            tokens,
            color: None,
            condition: None,
            is_date: false,
            percent: 0,
            scale: 0,
            thousands: false,
            exponent: None,
            fraction: None,
        }
    }

    /// Tokens of the integer part, the fraction part (with the point) and
    /// the exponent part.
    pub fn split_parts(&self) -> (&[Token], &[Token], &[Token]) {
        let exp_at = self.tokens.iter().position(|t| matches!(t, Token::Exp));
        let (body, exp) = match exp_at {
            Some(i) => (&self.tokens[..i], &self.tokens[i + 1..]),
            None => (&self.tokens[..], &[][..]),
        };
        match body.iter().position(|t| matches!(t, Token::Point)) {
            Some(p) => (&body[..p], &body[p..], exp),
            None => {
                // Literals after the last placeholder are a suffix: keep them
                // in the "fraction" part so they print after the digits.
                let last = body.iter().rposition(Token::is_digit).map_or(body.len(), |i| i + 1);
                (&body[..last], &body[last..], exp)
            }
        }
    }

    /// Tokens of a fraction's whole part, numerator and denominator.
    pub fn fraction_parts(&self) -> (&[Token], &[Token], &[Token]) {
        let slash = self.tokens.iter().position(|t| matches!(t, Token::Slash)).unwrap_or(0);
        let mut start = slash;
        while start > 0 && self.tokens[start - 1].is_digit() {
            start -= 1;
        }
        (&self.tokens[..start], &self.tokens[start..slash], &self.tokens[slash + 1..])
    }

    /// Decimal places shown.
    pub fn decimals(&self) -> usize {
        let (_, frac, _) = self.split_parts();
        frac.iter().filter(|t| t.is_digit()).count()
    }
}

/// The section used when a format has none: General.
pub static GENERAL_SECTION: LazyLock<Section> = LazyLock::new(|| Section::new(vec![Token::General]));

/// What Excel shows when no conditional section matches: a cell of hashes.
pub static HASHES_SECTION: LazyLock<Section> =
    LazyLock::new(|| Section::new(vec![Token::Literal("#".repeat(11))]));

fn named_color(name: &str) -> Option<u32> {
    Some(match name.to_ascii_lowercase().as_str() {
        "black" => 0x000000,
        "blue" => 0x0000FF,
        "cyan" => 0x00FFFF,
        "green" => 0x00FF00,
        "magenta" => 0xFF00FF,
        "red" => 0xFF0000,
        "white" => 0xFFFFFF,
        "yellow" => 0xFFFF00,
        _ => return None,
    })
}

/// Excel's default 56-colour palette, for `[ColorN]`.
pub const PALETTE: [u32; 56] = [
    0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x800000, 0x008000, 0x000080,
    0x808000, 0x800080, 0x008080, 0xC0C0C0, 0x808080, 0x9999FF, 0x993366, 0xFFFFCC, 0xCCFFFF, 0x660066, 0xFF8080,
    0x0066CC, 0xCCCCFF, 0x000080, 0xFF00FF, 0xFFFF00, 0x00FFFF, 0x800080, 0x800000, 0x008080, 0x0000FF, 0x00CCFF,
    0xCCFFFF, 0xCCFFCC, 0xFFFF99, 0x99CCFF, 0xFF99CC, 0xCC99FF, 0xFFCC99, 0x3366FF, 0x33CCCC, 0x99CC00, 0xFFCC00,
    0xFF9900, 0xFF6600, 0x666699, 0x969696, 0x003366, 0x339966, 0x003300, 0x333300, 0x993300, 0x993366, 0x333399,
    0x333333,
];

/// Split a code at unquoted, unescaped semicolons.
fn split_sections(code: &str) -> Vec<String> {
    let mut sections = Vec::new();
    let mut current = String::new();
    let mut chars = code.chars().peekable();
    let mut in_quote = false;
    let mut in_bracket = false;
    while let Some(c) = chars.next() {
        match c {
            '"' if !in_bracket => {
                in_quote = !in_quote;
                current.push(c);
            }
            '\\' | '_' | '*' if !in_quote && !in_bracket => {
                current.push(c);
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '[' if !in_quote => {
                in_bracket = true;
                current.push(c);
            }
            ']' if !in_quote => {
                in_bracket = false;
                current.push(c);
            }
            ';' if !in_quote && !in_bracket => sections.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    sections.push(current);
    sections
}

pub fn parse_sections(code: &str) -> Vec<Section> {
    let code = code.trim_matches(|c: char| c == '\u{FEFF}');
    let mut sections: Vec<Section> = split_sections(code).iter().map(|s| parse_section(s)).collect();
    // A trailing empty section ("0;-0;") still counts as a section that shows
    // nothing; a format that is entirely empty is General.
    if sections.len() == 1 && sections[0].tokens.is_empty() && sections[0].color.is_none() {
        sections[0].tokens.push(Token::General);
    }
    sections.truncate(4);
    sections
}

fn parse_bracket(content: &str, section: &mut Section, tokens: &mut Vec<Token>) {
    let lower = content.to_ascii_lowercase();
    if let Some(color) = named_color(content) {
        section.color = Some(color);
        return;
    }
    if let Some(n) = lower.strip_prefix("color") {
        if let Ok(i) = n.trim().parse::<usize>() {
            if (1..=56).contains(&i) {
                section.color = Some(PALETTE[i - 1]);
            }
        }
        return;
    }
    let ops = [("<=", CmpOp::Le), (">=", CmpOp::Ge), ("<>", CmpOp::Ne), ("<", CmpOp::Lt), (">", CmpOp::Gt), ("=", CmpOp::Eq)];
    for (prefix, op) in ops {
        if let Some(rest) = content.strip_prefix(prefix) {
            if let Ok(value) = rest.trim().parse::<f64>() {
                section.condition = Some(Condition { op, value });
            }
            return;
        }
    }
    if let Some(rest) = content.strip_prefix('$') {
        // [$₹-4009]: a currency symbol and a locale id.
        let symbol = rest.split('-').next().unwrap_or("");
        if !symbol.is_empty() {
            tokens.push(Token::Literal(symbol.to_owned()));
        }
        return;
    }
    let len = content.len().min(255) as u8;
    match lower.chars().next() {
        Some('h') if lower.chars().all(|c| c == 'h') => tokens.push(Token::ElapsedHours(len)),
        Some('m') if lower.chars().all(|c| c == 'm') => tokens.push(Token::ElapsedMinutes(len)),
        Some('s') if lower.chars().all(|c| c == 's') => tokens.push(Token::ElapsedSeconds(len)),
        // [DBNum1], [natnum1], [$-409] variants and the like: no effect here.
        _ => {}
    }
}

fn parse_section(text: &str) -> Section {
    let mut section = Section::new(Vec::new());
    let mut tokens: Vec<Token> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let push_lit = |tokens: &mut Vec<Token>, s: &str| {
        if let Some(Token::Literal(last)) = tokens.last_mut() {
            last.push_str(s);
        } else {
            tokens.push(Token::Literal(s.to_owned()));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let rest: String = chars[i..].iter().collect();
        let lower_rest = rest.to_ascii_lowercase();
        match c {
            '"' => {
                let end = chars[i + 1..].iter().position(|&c| c == '"').map_or(chars.len(), |p| i + 1 + p);
                let lit: String = chars[i + 1..end].iter().collect();
                push_lit(&mut tokens, &lit);
                i = end + 1;
                continue;
            }
            '\\' => {
                if let Some(&next) = chars.get(i + 1) {
                    push_lit(&mut tokens, &next.to_string());
                }
                i += 2;
                continue;
            }
            '_' => {
                tokens.push(Token::Space(chars.get(i + 1).copied().unwrap_or(' ')));
                i += 2;
                continue;
            }
            '*' => {
                tokens.push(Token::Fill(chars.get(i + 1).copied().unwrap_or(' ')));
                i += 2;
                continue;
            }
            '[' => {
                let end = chars[i + 1..].iter().position(|&c| c == ']').map_or(chars.len(), |p| i + 1 + p);
                let content: String = chars[i + 1..end].iter().collect();
                parse_bracket(&content, &mut section, &mut tokens);
                i = end + 1;
                continue;
            }
            _ => {}
        }
        if lower_rest.starts_with("general") {
            tokens.push(Token::General);
            i += 7;
            continue;
        }
        if lower_rest.starts_with("am/pm") {
            tokens.push(Token::AmPm(if rest.starts_with("AM") { AmPm::Upper } else { AmPm::Lower }));
            i += 5;
            continue;
        }
        if lower_rest.starts_with("a/p") {
            tokens.push(Token::AmPm(if rest.starts_with('A') { AmPm::ShortUpper } else { AmPm::ShortLower }));
            i += 3;
            continue;
        }
        let run = |ch: char| chars[i..].iter().take_while(|&&x| x.eq_ignore_ascii_case(&ch)).count();
        match c.to_ascii_lowercase() {
            '0' => tokens.push(Token::Zero),
            '#' => tokens.push(Token::Hash),
            '?' => tokens.push(Token::Question),
            '.' => {
                // ".0" after seconds is fractional seconds.
                let zeros = chars[i + 1..].iter().take_while(|&&x| x == '0').count();
                let after_seconds = tokens
                    .iter()
                    .rev()
                    .find(|t| !matches!(t, Token::Literal(_)))
                    .is_some_and(|t| matches!(t, Token::Second(_) | Token::ElapsedSeconds(_)));
                if zeros > 0 && after_seconds {
                    tokens.push(Token::SubSecond(zeros.min(3) as u8));
                    i += 1 + zeros;
                    continue;
                }
                tokens.push(Token::Point);
            }
            ',' => tokens.push(Token::Comma),
            '%' => {
                section.percent += 1;
                push_lit(&mut tokens, "%");
            }
            '@' => tokens.push(Token::At),
            '/' => tokens.push(Token::Slash),
            'e' if matches!(chars.get(i + 1), Some('+') | Some('-')) => {
                section.exponent = Some(ExpSpec {
                    upper: c == 'E',
                    plus: chars[i + 1] == '+',
                });
                tokens.push(Token::Exp);
                i += 2;
                continue;
            }
            'y' => {
                let n = run('y');
                tokens.push(Token::Year(n.min(4) as u8));
                i += n;
                continue;
            }
            'm' => {
                let n = run('m');
                tokens.push(Token::Month(n.min(5) as u8));
                i += n;
                continue;
            }
            'd' => {
                let n = run('d');
                tokens.push(Token::Day(n.min(4) as u8));
                i += n;
                continue;
            }
            'h' => {
                let n = run('h');
                tokens.push(Token::Hour(n.min(2) as u8));
                i += n;
                continue;
            }
            's' => {
                let n = run('s');
                tokens.push(Token::Second(n.min(2) as u8));
                i += n;
                continue;
            }
            'b' if matches!(chars.get(i + 1), Some('1') | Some('2')) => {
                // Buddhist / Hijri calendar switches: not supported, ignored.
                i += 2;
                continue;
            }
            _ => push_lit(&mut tokens, &c.to_string()),
        }
        i += 1;
    }
    finish_section(section, tokens)
}

fn finish_section(mut section: Section, mut tokens: Vec<Token>) -> Section {
    section.is_date = tokens.iter().any(Token::is_date);
    if section.is_date {
        // "m" next to hours or seconds means minutes.
        let date_positions: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| t.is_date())
            .map(|(i, _)| i)
            .collect();
        for (k, &pos) in date_positions.iter().enumerate() {
            if let Token::Month(len) = tokens[pos] {
                let prev = k.checked_sub(1).map(|j| &tokens[date_positions[j]]);
                let next = date_positions.get(k + 1).map(|&j| &tokens[j]);
                let after_hour = matches!(prev, Some(Token::Hour(_)) | Some(Token::ElapsedHours(_)));
                let before_second = matches!(next, Some(Token::Second(_)) | Some(Token::ElapsedSeconds(_)));
                if (after_hour || before_second) && len <= 2 {
                    tokens[pos] = Token::Minute(len);
                }
            }
        }
        // In dates, the fraction slash, the point and digit placeholders are
        // just characters.
        for token in tokens.iter_mut() {
            match token {
                Token::Comma => *token = Token::Literal(",".into()),
                Token::Slash => *token = Token::Literal("/".into()),
                Token::Point => *token = Token::Literal(".".into()),
                Token::Zero => *token = Token::Literal("0".into()),
                _ => {}
            }
        }
        section.tokens = merge_literals(tokens);
        return section;
    }

    // Commas: a run of them right after a digit placeholder either sits
    // between integer placeholders (a thousands separator) or ends the
    // number (scaling by 1000 each). Any other comma is literal.
    let point = tokens
        .iter()
        .position(|t| matches!(t, Token::Point | Token::Exp))
        .unwrap_or(tokens.len());
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == Token::Comma {
            let mut j = i;
            while j < tokens.len() && tokens[j] == Token::Comma {
                j += 1;
            }
            let prev_digit = out.last().is_some_and(Token::is_digit);
            let next_digit = tokens.get(j).is_some_and(Token::is_digit);
            if prev_digit && next_digit {
                if i < point {
                    section.thousands = true;
                }
            } else if prev_digit {
                section.scale += j - i;
            } else {
                for _ in i..j {
                    out.push(Token::Literal(",".into()));
                }
            }
            i = j;
            continue;
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    let mut tokens = out;

    // Fractions: digits after the slash written as literals fix the denominator.
    if let Some(slash) = tokens.iter().position(|t| matches!(t, Token::Slash)) {
        let has_numerator = slash > 0 && tokens[slash - 1].is_digit();
        if has_numerator {
            let mut fixed = String::new();
            let mut j = slash + 1;
            if let Some(Token::Literal(l)) = tokens.get(j) {
                let digits: String = l.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    fixed = digits.clone();
                    let rest = l[digits.len()..].to_owned();
                    if rest.is_empty() {
                        tokens.remove(j);
                    } else {
                        tokens[j] = Token::Literal(rest);
                    }
                    j = slash + 1;
                }
            }
            let den_digits = tokens[j..].iter().take_while(|t| t.is_digit()).count();
            section.fraction = Some(FractionSpec {
                denominator_digits: den_digits.max(fixed.len()),
                fixed_denominator: fixed.parse().ok().filter(|&d: &u64| d > 0),
            });
            if section.fraction.is_some_and(|f| f.fixed_denominator.is_some()) {
                // Represent the fixed denominator's width with placeholders.
                for _ in 0..fixed.len() {
                    tokens.insert(slash + 1, Token::Question);
                }
            }
        } else {
            tokens[slash] = Token::Literal("/".into());
        }
    }
    section.tokens = merge_literals(tokens);
    section
}

fn merge_literals(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    for token in tokens {
        if let (Some(Token::Literal(prev)), Token::Literal(next)) = (out.last_mut(), &token) {
            prev.push_str(next);
            continue;
        }
        out.push(token);
    }
    out
}

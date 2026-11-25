//! 命令面板计算器（官网 page/24）：`=表达式` 求 JS `Math` 风格的值。
//!
//! 这里是纯求值层：不认识面板、不认识终端、不写 PTY。输入是以 `=` 开头的
//! 查询串（前缀由调用方剥掉），输出是显示用的字符串或可读错误。
//!
//! 语法：四则 + 取余 + `^`（右结合）+ 一元正负 + 括号 + 逗号参数；
//! 名字支持 page/24 列出的常量与函数，函数名大小写敏感（与 JS 一致）。
//!
//! # 纯度契约（2026-11-25 明确登记）
//!
//! 本模块**不是纯函数模块**：`random()` 按 JS `Math.random` 语义每次都要变，
//! 而这里没有随机数生成器，只能读宿主墙钟。这一处副作用是有意保留的，写在
//! 契约里而不是藏在「纯求值层」这句话后面。需要确定性（单测 / 回放 / 宿主
//! 自己掌握随机源）时走 [`evaluate_with_seed`]：相同的表达式 + 种子必得相同
//! 结果，墙钟不会被读到；[`evaluate`] 是宿主便利入口，注入当前纳秒。

use std::collections::HashMap;
use std::fmt;

/// 计算器的分类错误。`Display` 直接给出官方 GIF 风格的整句提示，
/// UI 不必自己拼 "Invalid formula:"，也不必把错误拍平成 String 边界。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalcError {
    Empty,
    UnexpectedNumber(String),
    UnexpectedChar(char),
    UnexpectedToken(String),
    ExpectedClosingParen,
    UnexpectedEnd,
    TrailingInput,
    UnknownName(String),
    UnknownFunction(String),
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let detail = match self {
            CalcError::Empty => "empty".to_string(),
            CalcError::UnexpectedNumber(n) => format!("Unexpected number {n}"),
            CalcError::UnexpectedChar(c) => format!("Unexpected character {c:?}"),
            CalcError::UnexpectedToken(t) => format!("Unexpected token {t}"),
            CalcError::ExpectedClosingParen => "Expected ')'".to_string(),
            CalcError::UnexpectedEnd => "Unexpected end of formula".to_string(),
            CalcError::TrailingInput => "unexpected trailing input".to_string(),
            CalcError::UnknownName(n) => format!("Unknown name {n}"),
            CalcError::UnknownFunction(n) => format!("Unknown function {n}"),
        };
        write!(f, "Invalid formula: {detail}")
    }
}

/// 查询是否是计算器输入。调用方据此切到计算器 UI，不走命令搜索。
pub fn is_calculator_query(query: &str) -> bool {
    query.trim_start().starts_with('=')
}

/// 剥掉 `=` 与前后空白，留下表达式正文。
pub fn expression(query: &str) -> &str {
    query.trim_start().trim_start_matches('=').trim()
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Name(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    LParen,
    RParen,
    Comma,
}

/// 词法分析。非法字符与半截数字都在这里变成 `Err`，不把 panic 留给后面。
fn lex(src: &str) -> Result<Vec<Token>, CalcError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i < chars.len() && chars[i] == '.' {
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i < chars.len() && matches!(chars[i], 'e' | 'E') {
                let exp_start = i;
                i += 1;
                if i < chars.len() && matches!(chars[i], '+' | '-') {
                    i += 1;
                }
                let digits = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                if i == digits {
                    i = exp_start; // 半截指数：退回，让后面的字母变成独立 token
                }
            }
            let text: String = chars[start..i].iter().collect();
            let value = text
                .parse::<f64>()
                .map_err(|_| CalcError::UnexpectedNumber(text.clone()))?;
            out.push(Token::Number(value));
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '_' | '$'))
            {
                i += 1;
            }
            out.push(Token::Name(chars[start..i].iter().collect()));
            continue;
        }
        let tok = match c {
            '+' => Token::Plus,
            '-' => Token::Minus,
            '*' => Token::Star,
            '/' => Token::Slash,
            '%' => Token::Percent,
            '^' => Token::Caret,
            '(' => Token::LParen,
            ')' => Token::RParen,
            ',' => Token::Comma,
            _ => return Err(CalcError::UnexpectedChar(c)),
        };
        out.push(tok);
        i += 1;
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// 宿主注入的随机种子；`evaluate_with_seed` 一路传到这里。
    seed: u64,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn eat(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expr(&mut self) -> Result<f64, CalcError> {
        let mut v = self.term()?;
        loop {
            match self.peek() {
                Some(Token::Plus) => {
                    self.eat();
                    v += self.term()?;
                }
                Some(Token::Minus) => {
                    self.eat();
                    v -= self.term()?;
                }
                _ => return Ok(v),
            }
        }
    }

    fn term(&mut self) -> Result<f64, CalcError> {
        let mut v = self.unary()?;
        loop {
            match self.peek() {
                Some(Token::Star) => {
                    self.eat();
                    v *= self.unary()?;
                }
                Some(Token::Slash) => {
                    self.eat();
                    v /= self.unary()?;
                }
                Some(Token::Percent) => {
                    self.eat();
                    v %= self.unary()?;
                }
                _ => return Ok(v),
            }
        }
    }

    fn unary(&mut self) -> Result<f64, CalcError> {
        match self.peek() {
            Some(Token::Minus) => {
                self.eat();
                Ok(-self.unary()?)
            }
            Some(Token::Plus) => {
                self.eat();
                self.unary()
            }
            _ => self.power(),
        }
    }

    /// `^` 右结合：`2^3^2 = 2^(3^2) = 512`。
    fn power(&mut self) -> Result<f64, CalcError> {
        let base = self.primary()?;
        if matches!(self.peek(), Some(Token::Caret)) {
            self.eat();
            Ok(base.powf(self.unary()?))
        } else {
            Ok(base)
        }
    }

    fn primary(&mut self) -> Result<f64, CalcError> {
        match self.eat() {
            Some(Token::Number(n)) => Ok(n),
            Some(Token::LParen) => {
                let v = self.expr()?;
                match self.eat() {
                    Some(Token::RParen) => Ok(v),
                    _ => Err(CalcError::ExpectedClosingParen),
                }
            }
            Some(Token::Name(name)) => self.call_or_const(&name),
            other => Err(match other {
                Some(t) => CalcError::UnexpectedToken(format!("{t:?}")),
                None => CalcError::UnexpectedEnd,
            }),
        }
    }

    fn call_or_const(&mut self, name: &str) -> Result<f64, CalcError> {
        if !matches!(self.peek(), Some(Token::LParen)) {
            return constant(name).ok_or_else(|| CalcError::UnknownName(name.to_string()));
        }
        self.eat(); // '('
        let mut args = Vec::new();
        if !matches!(self.peek(), Some(Token::RParen)) {
            loop {
                args.push(self.expr()?);
                if matches!(self.peek(), Some(Token::Comma)) {
                    self.eat();
                    continue;
                }
                break;
            }
        }
        match self.eat() {
            Some(Token::RParen) => {}
            _ => return Err(CalcError::ExpectedClosingParen),
        }
        function(name, &args, Some(self.seed))
            .ok_or_else(|| CalcError::UnknownFunction(name.to_string()))
    }
}

/// page/24 列出的全部 Value Properties。
fn constant(name: &str) -> Option<f64> {
    let v = match name {
        "E" => std::f64::consts::E,
        "LN10" => std::f64::consts::LN_10,
        "LN2" => std::f64::consts::LN_2,
        "LOG2E" => std::f64::consts::LOG2_E,
        "LOG10E" => std::f64::consts::LOG10_E,
        "PI" => std::f64::consts::PI,
        "SQRT1_2" => std::f64::consts::FRAC_1_SQRT_2,
        "SQRT2" => std::f64::consts::SQRT_2,
        _ => return None,
    };
    Some(v)
}

/// page/24 列出的全部 Function Properties。`random()` 的随机源由宿主通过
/// `seed` 注入（`None` 时才读墙钟）；其余都是确定性数学函数。
fn function(name: &str, args: &[f64], seed: Option<u64>) -> Option<f64> {
    let a = |i: usize| args.get(i).copied().unwrap_or(f64::NAN);
    let out = match name {
        "abs" => a(0).abs(),
        "acos" => a(0).acos(),
        "acosh" => a(0).acosh(),
        "asin" => a(0).asin(),
        "asinh" => a(0).asinh(),
        "atan" => a(0).atan(),
        "atanh" => a(0).atanh(),
        "atan2" => a(0).atan2(a(1)),
        "cbrt" => a(0).cbrt(),
        "ceil" => a(0).ceil(),
        "clz32" => (a(0) as u32).leading_zeros() as f64,
        "cos" => a(0).cos(),
        "cosh" => a(0).cosh(),
        "exp" => a(0).exp(),
        "expm1" => a(0).exp_m1(),
        "floor" => a(0).floor(),
        "fround" => (a(0) as f32) as f64,
        "hypot" => a(0).hypot(a(1)),
        "imul" => (a(0) as i32).wrapping_mul(a(1) as i32) as f64,
        "log" => a(0).ln(),
        "log10" => a(0).log10(),
        "log1p" => a(0).ln_1p(),
        "log2" => a(0).log2(),
        "max" => args.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        "min" => args.iter().copied().fold(f64::INFINITY, f64::min),
        "pow" => a(0).powf(a(1)),
        "random" => random_unit(seed),
        "round" => a(0).round(),
        "sign" => sign(a(0)),
        "sin" => a(0).sin(),
        "sinh" => a(0).sinh(),
        "sqrt" => a(0).sqrt(),
        "tan" => a(0).tan(),
        "tanh" => a(0).tanh(),
        "trunc" => a(0).trunc(),
        _ => return None,
    };
    Some(out)
}

fn sign(v: f64) -> f64 {
    if v.is_nan() || v == 0.0 {
        v
    } else if v > 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// [0,1) 的均匀近似。有注入种子时只读种子（纯）；`None` 时由宿主入口注入
/// 墙钟纳秒——这是本模块唯一的非纯读，见模块头契约。
fn random_unit(seed: Option<u64>) -> f64 {
    let nanos = seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 + d.as_secs().wrapping_mul(1_000_000_003))
            .unwrap_or(0)
    });
    (nanos % 1_000_000_000) as f64 / 1_000_000_000.0
}

/// 求值并以显示字符串返回。`Err` 的文本以 `Invalid formula:` 开头，
/// 与官方 GIF 里的错误提示一致。随机源由宿主注入；本便利入口用墙钟纳秒。
pub fn evaluate(expression: &str) -> Result<String, CalcError> {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 + d.as_secs().wrapping_mul(1_000_000_003))
        .unwrap_or(0);
    evaluate_with_seed(expression, seed)
}

/// 确定性求值入口：`random()` 只读注入的种子，整条链上没有任何墙钟/全局状态。
pub fn evaluate_with_seed(expression: &str, seed: u64) -> Result<String, CalcError> {
    if expression.trim().is_empty() {
        return Err(CalcError::Empty);
    }
    let tokens = lex(expression)?;
    let mut parser = Parser { tokens, pos: 0, seed };
    let value = parser.expr()?;
    if parser.pos != parser.tokens.len() {
        return Err(CalcError::TrailingInput);
    }
    Ok(format_number(value))
}

/// JS 风格的最短十进制：整数不带 `.0`，极大/极小用科学计数。
fn format_number(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e21 {
        return format!("{v:.0}");
    }
    format!("{v}")
}

/// 供面板做「函数名补全」的名字表（page/24 的 Properties 全表）。
pub fn suggestions() -> &'static [&'static str] {
    &[
        "E", "LN10", "LN2", "LOG2E", "LOG10E", "PI", "SQRT1_2", "SQRT2", "abs", "acos",
        "acosh", "asin", "asinh", "atan", "atanh", "atan2", "cbrt", "ceil", "clz32", "cos",
        "cosh", "exp", "expm1", "floor", "fround", "hypot", "imul", "log", "log10", "log1p",
        "log2", "max", "min", "pow", "random", "round", "sign", "sin", "sinh", "sqrt", "tan",
        "tanh", "trunc",
    ]
}

/// 名字是函数还是需要参数的函数（用于面板补全时自动补左括号）。
pub fn is_function(name: &str) -> bool {
    constant(name).is_none() && suggestions().contains(&name)
}

/// 常量/函数的说明，补全列表与官方 GIF 的右侧描述列同源。
pub fn description(name: &str) -> Option<&'static str> {
    let table: HashMap<&str, &str> = [
        ("PI", "ratio of a circle's circumference to its diameter"),
        ("E", "Euler's number"),
        ("SQRT2", "square root of 2"),
        ("sin", "sine of a number"),
        ("cos", "cosine of a number"),
        ("sqrt", "square root of a number"),
        ("pow", "base raised to an exponent"),
        ("max", "largest of the given numbers"),
        ("min", "smallest of the given numbers"),
    ]
    .into_iter()
    .collect();
    table.get(name).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_detection_and_expression_extraction() {
        assert!(is_calculator_query("=1+2"));
        assert!(is_calculator_query("  =PI"));
        assert!(!is_calculator_query("Find"));
        assert_eq!(expression("  = 2+3 "), "2+3");
    }

    #[test]
    fn arithmetic_precedence_and_associativity() {
        assert_eq!(evaluate("1+2*3").as_deref(), Ok("7"));
        assert_eq!(evaluate("(1+2)*3").as_deref(), Ok("9"));
        assert_eq!(evaluate("-2^2").as_deref(), Ok("-4"));
        assert_eq!(evaluate("2^3^2").as_deref(), Ok("512"));
        assert_eq!(evaluate("7%4").as_deref(), Ok("3"));
    }

    #[test]
    fn all_documented_constants_and_functions_are_available() {
        for name in suggestions() {
            let expr = if constant(name).is_some() {
                name.to_string()
            } else if *name == "max" || *name == "min" || *name == "hypot" || *name == "pow"
                || *name == "atan2" || *name == "imul"
            {
                format!("{name}(2,3)")
            } else if *name == "random" {
                continue; // 每次不同，单独测
            } else {
                format!("{name}(1)")
            };
            assert!(evaluate(&expr).is_ok(), "{expr} 应当可求值");
        }
        assert_eq!(evaluate("sin(PI/2)").as_deref(), Ok("1"));
        assert_eq!(evaluate("sqrt(4)+abs(-3)").as_deref(), Ok("5"));
    }

    #[test]
    fn invalid_formulas_get_readable_errors() {
        assert!(evaluate("").unwrap_err().to_string().starts_with("Invalid formula:"));
        assert!(evaluate("1+").unwrap_err().to_string().contains("Unexpected"));
        assert!(evaluate("(1+2").unwrap_err().to_string().contains("')'"));
        assert!(evaluate("1 2").unwrap_err().to_string().contains("trailing"));
        assert!(evaluate("nope(1)").unwrap_err().to_string().contains("Unknown function"));
    }

    #[test]
    fn seeded_evaluation_is_deterministic_and_never_reads_the_clock() {
        let a = evaluate_with_seed("random()", 12345).unwrap();
        assert_eq!(a, evaluate_with_seed("random()", 12345).unwrap());
        assert_ne!(a, evaluate_with_seed("random()", 999).unwrap());
        let value: f64 = a.parse().unwrap();
        assert!((0.0..1.0).contains(&value));
        // 注入种子后连普通函数也走同一条确定性路径。
        assert_eq!(evaluate_with_seed("sin(PI/2)", 7).as_deref(), Ok("1"));
        assert!(evaluate_with_seed("", 7).unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn random_changes_between_calls_and_stays_in_range() {
        let a = evaluate("random()").unwrap().parse::<f64>().unwrap();
        let b = evaluate("random()").unwrap().parse::<f64>().unwrap();
        assert!((0.0..1.0).contains(&a) && (0.0..1.0).contains(&b));
        assert_ne!(a, b, "两次调用必须真的不同");
    }
}

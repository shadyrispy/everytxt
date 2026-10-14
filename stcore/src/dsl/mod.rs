//! DSL parser（nom 8 —— 组合器是 parser 值而非函数，统一 .parse(i) 调用）
//!
//! 搜索语法：
//!   query    := or_expr
//!   or_expr  := and_expr ( '|' and_expr )*        // 优先级最低
//!   and_expr := not_expr ( '&'? not_expr )*       // 后续项的 '&' 可选（隐式 AND）
//!   not_expr := '!' not_expr | atom                // 优先级最高
//!   atom     := '(' or_expr ')' | '"' text '"' | word
//!   word     := 非空白且非 & | ! ( ) " 的连续字符
//!
//! 设计决策（S8 spike 固化口径）：
//!   - `()` 空分组 → 拒绝
//!   - `""` 空引号 → 拒绝
//!   - `a ! b` 缺运算符 → 隐式 AND 合法
//!   - 空/纯空白查询 → Expr::Empty（不匹配任何文档）
//!   - 运算符两侧空白可选（a&b 合法）
//!   - NOT > AND > OR 优先级

use nom::{
    bytes::complete::take_until,
    character::complete::{char, multispace0, none_of},
    combinator::opt,
    multi::many0,
    sequence::{delimited, pair, preceded, terminated},
    IResult, Parser,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Or(Vec<Expr>),
    And(Vec<Expr>),
    Not(Box<Expr>),
    /// text=词面；exact=true 表示引号包裹（完全匹配）
    Term {
        text: String,
        exact: bool,
    },
    /// 空查询
    Empty,
}

impl Expr {
    /// 收集所有词面并带 exact 标志（供高亮 token 按语义切分：
    /// fuzzy 词面走无缝切分与 tantivy 查询映射同源，exact 引号词用整串）
    pub fn collect_terms_tagged(&self, out: &mut Vec<(String, bool)>) {
        match self {
            Expr::Or(v) | Expr::And(v) => v.iter().for_each(|e| e.collect_terms_tagged(out)),
            Expr::Not(e) => e.collect_terms_tagged(out),
            Expr::Term { text, exact } => out.push((text.clone(), *exact)),
            Expr::Empty => {}
        }
    }
}

/// 嵌套深度上限：防深嵌套（`((((...` / `!!!!...`）递归解析爆栈，
/// 超限返回 ParseErr 而非栈溢出
pub const MAX_NEST_DEPTH: usize = 64;

/// 顶层入口：空查询 → Empty；其余走 or_expr，且必须消费完全部输入
pub fn parse_query(input: &str) -> Result<Expr, String> {
    if input.trim().is_empty() {
        return Ok(Expr::Empty);
    }
    match or_expr_d(input, 0) {
        Ok((rest, expr)) if rest.trim().is_empty() => Ok(expr),
        Ok((rest, _)) => Err(format!("意外输入 {:?}（是否缺少 & 或 | 连接符？）", rest)),
        Err(_) => Err(format!("语法错误或嵌套过深（上限 {MAX_NEST_DEPTH} 层）")),
    }
}

fn or_expr_d(i: &str, depth: usize) -> IResult<&str, Expr> {
    let (mut i, first) = and_expr_d(i, depth)?;
    let mut rest: Vec<(char, Expr)> = Vec::new();
    // 手写循环替代 many0(pair(ws_op('|'), closure))：nom 8 的组合器闭包无法
    // 表达 HRTB（`for<'a> Fn(&'a str) -> IResult<&'a str, _>`），捕获 depth 的
    // 递归解析器只能以显式循环收口
    while let Ok((j, _)) = ws_op('|').parse(i) {
        match and_expr_d(j, depth) {
            Ok((k, e)) => {
                rest.push(('|', e));
                i = k;
            }
            // '|' 后解析失败：回退整轮（不消费 '|'），交由顶层报"意外输入"
            Err(_) => break,
        }
    }
    Ok((i, fold(first, rest, Expr::Or)))
}

fn and_expr_d(i: &str, depth: usize) -> IResult<&str, Expr> {
    let (mut i, first) = not_expr_d(i, depth)?;
    let mut rest: Vec<(char, Expr)> = Vec::new();
    loop {
        // 隐式 AND：后续项前的 '&' 可选
        let save = i;
        // 显式绑定 IResult 以固定错误类型（nom 8 的 ParseError 有多个 impl，需标注）
        let amp: IResult<&str, Option<char>> = preceded(multispace0, opt(char('&'))).parse(save);
        let j = match amp {
            Ok((j, _)) => j,
            Err(_) => break,
        };
        // 项与项之间允许空白（原 many0 第二项为 preceded(multispace0, not_expr)）
        let ws: IResult<&str, &str> = multispace0.parse(j);
        let j = match ws {
            Ok((j, _)) => j,
            Err(_) => break,
        };
        match not_expr_d(j, depth) {
            Ok((k, e)) => {
                rest.push(('&', e));
                i = k;
            }
            // 本轮未成项 → 整轮回退到 save（含已消费的 '&'），保持 many0 语义
            Err(_) => {
                i = save;
                break;
            }
        }
    }
    Ok((i, fold(first, rest, Expr::And)))
}

fn not_expr_d(i: &str, depth: usize) -> IResult<&str, Expr> {
    if depth >= MAX_NEST_DEPTH {
        return Err(nom::Err::Error(nom::error::Error::new(
            i,
            nom::error::ErrorKind::TooLarge,
        )));
    }
    let bang: IResult<&str, char> = terminated(char('!'), multispace0).parse(i);
    if let Ok((j, _)) = bang {
        let (k, e) = not_expr_d(j, depth + 1)?;
        return Ok((k, Expr::Not(Box::new(e))));
    }
    atom_d(i, depth)
}

fn atom_d(i: &str, depth: usize) -> IResult<&str, Expr> {
    let lparen: IResult<&str, char> = terminated(char('('), multispace0).parse(i);
    if let Ok((j, _)) = lparen {
        let (j, e) = or_expr_d(j, depth + 1)?;
        let (j, _) = preceded(multispace0, char(')')).parse(j)?;
        return Ok((j, e));
    }
    if let Ok((j, s)) = quoted(i) {
        return Ok((
            j,
            Expr::Term {
                text: s.to_string(),
                exact: true,
            },
        ));
    }
    if let Ok((j, s)) = word(i) {
        return Ok((
            j,
            Expr::Term {
                text: s.to_string(),
                exact: false,
            },
        ));
    }
    Err(nom::Err::Error(nom::error::Error::new(
        i,
        nom::error::ErrorKind::Alt,
    )))
}

/// 引号串：非空（`""` 拒绝）
fn quoted(i: &str) -> IResult<&str, &str> {
    let (i, s) = delimited(char('"'), take_until("\""), char('"')).parse(i)?;
    if s.is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(
            "\"\"",
            nom::error::ErrorKind::Tag,
        )));
    }
    Ok((i, s))
}

/// 词面：至少一个非运算符、非空白字符
///
/// 必须 many1 语义（首字符 + 后续任意个）：若允许空匹配，and_expr 的
/// `many0(pair(opt('&'), not_expr))` 会以零消耗"成功"，nom 的 many0
/// 随即报 Many0 错误（spike 期实测：`a | b & !c` 整个解析失败）。
fn word(i: &str) -> IResult<&str, &str> {
    nom::combinator::recognize(pair(
        none_of(" \t\r\n&|!()\""),
        many0(none_of(" \t\r\n&|!()\"")),
    ))
    .parse(i)
}

/// 运算符两侧允许空白
fn ws_op(c: char) -> impl Fn(&str) -> IResult<&str, char> {
    move |i: &str| delimited(multispace0, char(c), multispace0).parse(i)
}

/// 二元折叠：单个操作数直接返回，多个包成 Or/And
fn fold(first: Expr, rest: Vec<(char, Expr)>, wrap: fn(Vec<Expr>) -> Expr) -> Expr {
    if rest.is_empty() {
        return first;
    }
    let mut v = vec![first];
    v.extend(rest.into_iter().map(|(_, e)| e));
    wrap(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(q: &str) -> Expr {
        parse_query(q).unwrap()
    }

    #[test]
    fn manual_screenshot_example() {
        // 隐式 AND 示例（缺 & 的写法）
        assert_eq!(
            t("test & (hello|\"this is\") !that"),
            Expr::And(vec![
                Expr::Term {
                    text: "test".into(),
                    exact: false
                },
                Expr::Or(vec![
                    Expr::Term {
                        text: "hello".into(),
                        exact: false
                    },
                    Expr::Term {
                        text: "this is".into(),
                        exact: true
                    },
                ]),
                Expr::Not(Box::new(Expr::Term {
                    text: "that".into(),
                    exact: false
                })),
            ])
        );
    }

    #[test]
    fn precedence_not_over_and_over_or() {
        // a | b & !c → Or(a, And(b, Not(c)))
        assert_eq!(
            t("a | b & !c"),
            Expr::Or(vec![
                Expr::Term {
                    text: "a".into(),
                    exact: false
                },
                Expr::And(vec![
                    Expr::Term {
                        text: "b".into(),
                        exact: false
                    },
                    Expr::Not(Box::new(Expr::Term {
                        text: "c".into(),
                        exact: false
                    })),
                ]),
            ])
        );
    }

    #[test]
    fn rejects() {
        assert!(parse_query("a &").is_err());
        assert!(parse_query("(a").is_err());
        assert!(parse_query("\"abc").is_err());
        assert!(parse_query("()").is_err());
        assert!(parse_query("\"\"").is_err());
        assert!(parse_query("!").is_err());
    }

    #[test]
    fn deep_nesting_rejected_not_stack_overflow() {
        // 超深嵌套必须返回 ParseErr（旧实现递归下降无深度限制，会栈溢出）
        let deep = format!("{}a{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(parse_query(&deep).is_err(), "超深括号应报错而非爆栈");
        // 64 层内仍可正常解析
        let ok = format!("{}a{}", "(".repeat(60), ")".repeat(60));
        assert!(parse_query(&ok).is_ok());
        // '!' 链同样受限
        let deep_not = format!("{}a", "!".repeat(100_000));
        assert!(parse_query(&deep_not).is_err());
    }

    #[test]
    fn empty_query() {
        assert_eq!(parse_query("   ").unwrap(), Expr::Empty);
    }
}

//! Regular-expression patterns for `~`.
//!
//! PostgreSQL executes the pattern; this validator only admits a small subset
//! that Rust's `regex-syntax` and PostgreSQL's ARE engine read alike, and that
//! cannot backtrack catastrophically: no backreferences, lookaround, inline
//! flags, directors or word anchors, bounded counted repetition, and no
//! quantifier nested under another (star height at most one).

use regex_syntax::ast::{
    self, Ast, AssertionKind, ClassSet, ClassSetItem, GroupKind, LiteralKind, RepetitionKind,
    RepetitionRange, SpecialLiteralKind,
};

pub(crate) const MAX_PATTERN_CHARS: usize = 128;
const MAX_REPEAT: u32 = 100;

pub(crate) fn validate(pattern: &str) -> Result<(), String> {
    let refuse = |why: &str| Err(format!("pattern '{pattern}': {why}"));
    if pattern.is_empty() {
        return refuse("the pattern is empty");
    }
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return refuse(&format!("patterns are limited to {MAX_PATTERN_CHARS} characters"));
    }
    if pattern.starts_with("***") {
        return refuse("'***' directors are not allowed");
    }
    let ast = ast::parse::ParserBuilder::new()
        .nest_limit(16)
        .build()
        .parse(pattern)
        .map_err(|error| format!("pattern '{pattern}': {}", error.kind()))?;
    walk(&ast, false).or_else(|why| refuse(why))
}

fn walk(ast: &Ast, repeated: bool) -> Result<(), &'static str> {
    match ast {
        Ast::Empty(_) | Ast::Dot(_) | Ast::ClassPerl(_) => Ok(()),
        Ast::Flags(_) => Err("inline flags are not allowed"),
        Ast::Literal(literal) => literal_ok(&literal.kind),
        Ast::Assertion(assertion) => match assertion.kind {
            AssertionKind::StartLine | AssertionKind::EndLine => Ok(()),
            _ => Err("only the ^ and $ anchors are allowed"),
        },
        Ast::ClassUnicode(_) => Err("Unicode classes are not allowed"),
        Ast::ClassBracketed(class) => class_ok(&class.kind),
        Ast::Repetition(repetition) => {
            if repeated {
                return Err("a quantifier cannot apply to an expression that already has one");
            }
            if let RepetitionKind::Range(range) = &repetition.op.kind {
                let (RepetitionRange::Exactly(n) | RepetitionRange::AtLeast(n) | RepetitionRange::Bounded(_, n)) = range;
                if *n > MAX_REPEAT {
                    return Err("counted repetition is limited to 100");
                }
            }
            walk(&repetition.ast, true)
        }
        Ast::Group(group) => match &group.kind {
            GroupKind::CaptureIndex(_) => walk(&group.ast, repeated),
            GroupKind::NonCapturing(flags) if flags.items.is_empty() => walk(&group.ast, repeated),
            GroupKind::NonCapturing(_) => Err("inline flags are not allowed"),
            GroupKind::CaptureName { .. } => Err("named groups are not allowed"),
        },
        Ast::Alternation(alternation) => alternation.asts.iter().try_for_each(|ast| walk(ast, repeated)),
        Ast::Concat(concat) => concat.asts.iter().try_for_each(|ast| walk(ast, repeated)),
    }
}

fn literal_ok(kind: &LiteralKind) -> Result<(), &'static str> {
    match kind {
        LiteralKind::Verbatim | LiteralKind::Meta => Ok(()),
        LiteralKind::Special(SpecialLiteralKind::Tab | SpecialLiteralKind::LineFeed | SpecialLiteralKind::CarriageReturn) => Ok(()),
        _ => Err("only plain characters, escaped metacharacters and \\t \\n \\r are allowed"),
    }
}

fn class_ok(set: &ClassSet) -> Result<(), &'static str> {
    match set {
        ClassSet::BinaryOp(_) => Err("class set operations are not allowed"),
        ClassSet::Item(item) => item_ok(item),
    }
}

fn item_ok(item: &ClassSetItem) -> Result<(), &'static str> {
    match item {
        ClassSetItem::Empty(_) | ClassSetItem::Perl(_) => Ok(()),
        ClassSetItem::Literal(literal) => literal_ok(&literal.kind),
        ClassSetItem::Range(range) => {
            literal_ok(&range.start.kind)?;
            literal_ok(&range.end.kind)
        }
        ClassSetItem::Ascii(class) if !class.negated => Ok(()),
        ClassSetItem::Ascii(_) => Err("negated POSIX classes are not allowed"),
        ClassSetItem::Unicode(_) => Err("Unicode classes are not allowed"),
        ClassSetItem::Bracketed(_) => Err("nested brackets are not allowed"),
        ClassSetItem::Union(union) => union.items.iter().try_for_each(item_ok),
    }
}

//! Parser helpers: operator tables and target analysis.

use super::*;

#[derive(PartialEq)]
pub(super) enum BinaryKind {
    Binary(BinaryOp),
    Logical(LogicalOp),
}

pub(super) fn binary_operator(token: &Token) -> Option<(u8, BinaryKind)> {
    let result = match &token.kind {
        TokenKind::Punct(punct) => match punct {
            Punct::Nullish => (1, BinaryKind::Logical(LogicalOp::Nullish)),
            Punct::Or => (2, BinaryKind::Logical(LogicalOp::Or)),
            Punct::And => (3, BinaryKind::Logical(LogicalOp::And)),
            Punct::BitOr => (4, BinaryKind::Binary(BinaryOp::BitOr)),
            Punct::BitXor => (5, BinaryKind::Binary(BinaryOp::BitXor)),
            Punct::BitAnd => (6, BinaryKind::Binary(BinaryOp::BitAnd)),
            Punct::Eq => (7, BinaryKind::Binary(BinaryOp::Eq)),
            Punct::Ne => (7, BinaryKind::Binary(BinaryOp::Ne)),
            Punct::StrictEq => (7, BinaryKind::Binary(BinaryOp::StrictEq)),
            Punct::StrictNe => (7, BinaryKind::Binary(BinaryOp::StrictNe)),
            Punct::Lt => (8, BinaryKind::Binary(BinaryOp::Lt)),
            Punct::Le => (8, BinaryKind::Binary(BinaryOp::Le)),
            Punct::Gt => (8, BinaryKind::Binary(BinaryOp::Gt)),
            Punct::Ge => (8, BinaryKind::Binary(BinaryOp::Ge)),
            Punct::Shl => (9, BinaryKind::Binary(BinaryOp::Shl)),
            Punct::Shr => (9, BinaryKind::Binary(BinaryOp::Shr)),
            Punct::UShr => (9, BinaryKind::Binary(BinaryOp::UShr)),
            Punct::Plus => (10, BinaryKind::Binary(BinaryOp::Add)),
            Punct::Minus => (10, BinaryKind::Binary(BinaryOp::Sub)),
            Punct::Star => (11, BinaryKind::Binary(BinaryOp::Mul)),
            Punct::Slash => (11, BinaryKind::Binary(BinaryOp::Div)),
            Punct::Percent => (11, BinaryKind::Binary(BinaryOp::Mod)),
            Punct::Exp => (12, BinaryKind::Binary(BinaryOp::Exp)),
            _ => return None,
        },
        TokenKind::Ident(name) if name == "in" => (8, BinaryKind::Binary(BinaryOp::In)),
        TokenKind::Ident(name) if name == "instanceof" => {
            (8, BinaryKind::Binary(BinaryOp::InstanceOf))
        }
        _ => return None,
    };
    Some(result)
}

pub(super) fn is_assignment_target(expression: &Expr) -> bool {
    matches!(
        expression.kind,
        ExprKind::Ident(_) | ExprKind::Member { .. } | ExprKind::Array(_) | ExprKind::Object(_)
    )
}

pub(super) fn property_name_string(name: &PropertyName) -> String {
    match name {
        PropertyName::Ident(value) | PropertyName::String(value) => value.clone(),
        PropertyName::Number(value) => {
            if value.fract() == 0.0 {
                format!("{}", *value as i64)
            } else {
                value.to_string()
            }
        }
        PropertyName::Computed(_) => "[computed]".into(),
    }
}

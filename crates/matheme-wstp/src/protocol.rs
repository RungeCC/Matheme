//! WSTP packet heads, kernel control symbols, and packet classification.

use matheme_wl::{Expr, Symbol, normal};

pub(crate) const EVALUATE_PACKET: Symbol = Symbol::from_static("System`EvaluatePacket");
pub(crate) const RETURN_PACKET: Symbol = Symbol::from_static("System`ReturnPacket");
pub(crate) const RETURN_TEXT_PACKET: Symbol = Symbol::from_static("System`ReturnTextPacket");
pub(crate) const RETURN_EXPRESSION_PACKET: Symbol =
    Symbol::from_static("System`ReturnExpressionPacket");
pub(crate) const INPUT_NAME_PACKET: Symbol = Symbol::from_static("System`InputNamePacket");
pub(crate) const OUTPUT_NAME_PACKET: Symbol = Symbol::from_static("System`OutputNamePacket");
pub(crate) const MESSAGE_PACKET: Symbol = Symbol::from_static("System`MessagePacket");
pub(crate) const TEXT_PACKET: Symbol = Symbol::from_static("System`TextPacket");
pub(crate) const TO_EXPRESSION: Symbol = Symbol::from_static("System`ToExpression");
pub(crate) const EXIT: Symbol = Symbol::from_static("System`Exit");

pub(crate) fn eval_packet_expr(expr: &Expr) -> Expr {
    normal!(&EVALUATE_PACKET, expr.clone())
}

pub(crate) fn to_expression_expr(wl_code: &str) -> Expr {
    normal!(&TO_EXPRESSION, wl_code)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PacketAction {
    Return(Expr),
    MessageHeader,
    Text(String),
    Prompt,
    Unexpected,
}

pub(crate) fn classify_packet(packet: &Expr) -> PacketAction {
    let Some(normal) = packet.as_normal() else {
        return PacketAction::Unexpected;
    };
    if (normal.has_head(&RETURN_PACKET)
        || normal.has_head(&RETURN_TEXT_PACKET)
        || normal.has_head(&RETURN_EXPRESSION_PACKET))
        && normal.len() == 1
    {
        return PacketAction::Return(normal.elements()[0].clone());
    }
    if normal.has_head(&MESSAGE_PACKET) {
        return PacketAction::MessageHeader;
    }
    if normal.has_head(&TEXT_PACKET) && normal.len() == 1 {
        let elem = &normal.elements()[0];
        let text = elem
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| elem.to_string());
        return PacketAction::Text(text);
    }
    if normal.has_head(&INPUT_NAME_PACKET) || normal.has_head(&OUTPUT_NAME_PACKET) {
        return PacketAction::Prompt;
    }
    PacketAction::Unexpected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_standard_and_variant_return_packets() {
        let ret = Expr::normal(&RETURN_PACKET, [Expr::from(42_i64)]);
        assert_eq!(
            classify_packet(&ret),
            PacketAction::Return(Expr::from(42_i64))
        );

        let ret_text = Expr::normal(&RETURN_TEXT_PACKET, [Expr::from("ok")]);
        assert_eq!(
            classify_packet(&ret_text),
            PacketAction::Return(Expr::from("ok"))
        );

        let ret_expr = Expr::normal(&RETURN_EXPRESSION_PACKET, [Expr::from(7_i64)]);
        assert_eq!(
            classify_packet(&ret_expr),
            PacketAction::Return(Expr::from(7_i64))
        );

        let msg = Expr::normal(
            &MESSAGE_PACKET,
            [Expr::symbol(Symbol::system("Power")), Expr::from("infy")],
        );
        assert_eq!(classify_packet(&msg), PacketAction::MessageHeader);

        let text = Expr::normal(&TEXT_PACKET, [Expr::from("hello")]);
        assert_eq!(classify_packet(&text), PacketAction::Text("hello".into()));

        let input_name = Expr::normal(&INPUT_NAME_PACKET, [Expr::from("In[1]:= ")]);
        assert_eq!(classify_packet(&input_name), PacketAction::Prompt);

        let other = Expr::from("raw");
        assert_eq!(classify_packet(&other), PacketAction::Unexpected);
    }

    #[test]
    fn builds_eval_and_to_expression_packets() {
        let eval_pkt = eval_packet_expr(&to_expression_expr("1 + 1"));
        let outer = eval_pkt.as_normal().expect("EvaluatePacket");
        assert!(outer.has_head(&EVALUATE_PACKET));
        let inner = outer.elements()[0].as_normal().expect("ToExpression");
        assert!(inner.has_head(&TO_EXPRESSION));
        assert_eq!(inner.elements()[0].as_str(), Some("1 + 1"));
    }
}

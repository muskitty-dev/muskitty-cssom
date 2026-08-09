//! `element.style` — 把 Element 的 `style` 属性当作
//! [`CssStyleDeclaration`] 读写。参见 CSSOM §4（`CSSStyleDeclaration`
//! 接口：`cssText` / `getPropertyValue` / `setProperty` / `removeProperty`）。
//!
//! # 设计：attribute 为唯一 source of truth
//!
//! Cascade 直接读取原始 style 属性字符串（`filter.rs collect_from_style_attr`），
//! 因此每次访问都执行 **parse → mutate → serialize → 写回 attribute**，
//! 无缓存对象、无陈旧状态。property 名按 CSS 语义做 ASCII 大小写不敏感
//! 匹配（CSS Syntax §6.3.4），css-parser / `get_property` 均不归一化，
//! 归一化在 trait 层完成。
//!
//! `ElementStyle` 实现于 `Rc<RefCell<Node>>`（本地 trait + 外部类型，
//! 无 orphan 问题），方法内部经 `RefCell` 借出节点并读写 attribute。

use std::cell::RefCell;
use std::rc::Rc;

use muskitty_dom::Node;

use crate::{parse_declaration_block, CssDeclaration, ToCss};

/// 把 Element 的 `style` 属性映射为可读写的 `CSSStyleDeclaration`。
///
/// 仅对 `NodeKind::Element` 有意义；对其他节点类型所有操作均为空操作
/// （读返回空串 / `None`，写不落盘）。
pub trait ElementStyle {
    /// `CSSStyleDeclaration.cssText` getter（CSSOM §4）。
    fn style_css_text(&self) -> String;

    /// `CSSStyleDeclaration.cssText` setter：整体重解析覆盖。
    fn set_style_css_text(&self, css: &str);

    /// `CSSStyleDeclaration.getPropertyValue(name)`：返回该属性的值文本
    /// （`serialize_component_values` 序列化）；不存在返回 `None`。
    fn style_property(&self, name: &str) -> Option<String>;

    /// `CSSStyleDeclaration.setProperty(name, value)`：设置属性。
    /// 已存在同名属性则覆盖其值；否则追加到末尾。空值等价于移除。
    fn set_style_property(&self, name: &str, value: &str);

    /// `CSSStyleDeclaration.removeProperty(name)`：移除属性。
    fn remove_style_property(&self, name: &str);
}

impl ElementStyle for Rc<RefCell<Node>> {
    fn style_css_text(&self) -> String {
        read_style_attr(self)
    }

    fn set_style_css_text(&self, css: &str) {
        write_style_attr(self, css);
    }

    fn style_property(&self, name: &str) -> Option<String> {
        let block = parse_declaration_block(&read_style_attr(self));
        let lower = name.to_ascii_lowercase();
        // 取最后一个同名声明（级联语义，见 §8.6 get_property）。
        let decl = block
            .declarations
            .iter()
            .rev()
            .find(|d| d.name.eq_ignore_ascii_case(&lower))?;
        Some(serialize_property_value(decl))
    }

    fn set_style_property(&self, name: &str, value: &str) {
        let lower = name.to_ascii_lowercase();
        if value.is_empty() {
            self.remove_style_property(&lower);
            return;
        }
        // parse → mutate → serialize → write-back
        let mut block = parse_declaration_block(&read_style_attr(self));
        match block
            .declarations
            .iter_mut()
            .rev()
            .find(|d| d.name.eq_ignore_ascii_case(&lower))
        {
            Some(decl) => decl.value = parse_value_tokens(value),
            None => block.push(CssDeclaration::new(lower, parse_value_tokens(value), false)),
        }
        write_style_attr(self, &block.to_css_string());
    }

    fn remove_style_property(&self, name: &str) {
        let lower = name.to_ascii_lowercase();
        let mut block = parse_declaration_block(&read_style_attr(self));
        let before = block.declarations.len();
        block
            .declarations
            .retain(|d| !d.name.eq_ignore_ascii_case(&lower));
        if block.declarations.len() != before {
            write_style_attr(self, &block.to_css_string());
        }
    }
}

/// 读取 style 属性原文（无则空串）。
fn read_style_attr(node: &Rc<RefCell<Node>>) -> String {
    node.borrow()
        .kind
        .as_element()
        .and_then(|e| e.get_attribute("style"))
        .map(str::to_string)
        .unwrap_or_default()
}

/// 写回 style 属性。空串 → 移除属性（CSSOM §4 语义：空声明块不落 attribute）。
fn write_style_attr(node: &Rc<RefCell<Node>>, css: &str) {
    let mut n = node.borrow_mut();
    if let Some(e) = n.kind.as_element_mut() {
        if css.is_empty() {
            e.remove_attribute("style");
        } else {
            e.set_attribute("style", css);
        }
    }
}

/// 把属性值字符串解析为 component value 列表。
fn parse_value_tokens(value: &str) -> Vec<crate::ComponentValue> {
    // 单个属性的值本身是一个小声明块；取其首条声明的 value。
    let block = parse_declaration_block(&format!("x: {value}"));
    block
        .declarations
        .into_iter()
        .next()
        .map(|d| d.value)
        .unwrap_or_default()
}

/// 属性值的 CSS 文本（§3 序列化规则）。
fn serialize_property_value(decl: &CssDeclaration) -> String {
    crate::serialize::serialize_component_values(&decl.value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use muskitty_dom::{Attribute, Node};

    /// 构造一个带 style attribute 的 div。
    fn styled_div(style_attr: &str) -> Rc<RefCell<Node>> {
        let doc = Node::new_document();
        let attrs = vec![Attribute::new("style", style_attr)];
        Node::new_element_html("div", attrs, &doc)
    }

    fn plain_div() -> Rc<RefCell<Node>> {
        let doc = Node::new_document();
        Node::new_element_html("div", vec![], &doc)
    }

    fn current_style(node: &Rc<RefCell<Node>>) -> String {
        node.borrow()
            .kind
            .as_element()
            .and_then(|e| e.get_attribute("style"))
            .map(str::to_string)
            .unwrap_or_default()
    }

    #[test]
    fn empty_attr_reads_empty_text() {
        let div = plain_div();
        assert_eq!(div.style_css_text(), "");
        assert_eq!(div.style_property("color"), None);
    }

    #[test]
    fn css_text_roundtrip_preserves_order() {
        let div = styled_div("color: red; font-size: 12px");
        assert_eq!(div.style_css_text(), "color: red; font-size: 12px");
    }

    #[test]
    fn set_css_text_replaces_entire_attribute() {
        let div = styled_div("color: red");
        div.set_style_css_text("margin: 0; padding: 1px");
        assert_eq!(current_style(&div), "margin: 0; padding: 1px");
    }

    #[test]
    fn set_css_text_empty_removes_attribute() {
        let div = styled_div("color: red");
        div.set_style_css_text("");
        assert_eq!(current_style(&div), "");
        // 属性物理移除
        let borrowed = div.borrow();
        let e = borrowed.kind.as_element().unwrap();
        assert_eq!(e.get_attribute("style"), None);
    }

    #[test]
    fn get_property_value_serialized() {
        let div = styled_div("color: red");
        assert_eq!(div.style_property("color"), Some("red".to_string()));
        assert_eq!(div.style_property("COLOR"), Some("red".to_string()));
    }

    #[test]
    fn set_property_creates_new_declaration() {
        let div = plain_div();
        div.set_style_property("color", "blue");
        // 块序列化每个声明后带分号（与浏览器 style attribute 一致）
        assert_eq!(current_style(&div), "color: blue;");
    }

    #[test]
    fn set_property_overwrites_last_same_name() {
        let div = styled_div("color: red");
        div.set_style_property("color", "green");
        assert_eq!(current_style(&div), "color: green;");
    }

    #[test]
    fn set_property_preserves_other_declarations() {
        let div = styled_div("color: red; margin: 0");
        div.set_style_property("color", "green");
        assert_eq!(current_style(&div), "color: green; margin: 0;");
    }

    #[test]
    fn set_property_empty_value_removes() {
        let div = styled_div("color: red; margin: 0");
        div.set_style_property("color", "");
        assert_eq!(current_style(&div), "margin: 0;");
    }

    #[test]
    fn remove_property_removes_and_drops_attr_when_empty() {
        let div = styled_div("color: red");
        div.remove_style_property("color");
        assert_eq!(current_style(&div), "");
        let borrowed = div.borrow();
        let e = borrowed.kind.as_element().unwrap();
        assert_eq!(e.get_attribute("style"), None);
    }

    #[test]
    fn remove_property_keeps_siblings() {
        let div = styled_div("color: red; margin: 0");
        div.remove_style_property("COLOR"); // 大小写不敏感
        assert_eq!(current_style(&div), "margin: 0;");
    }

    #[test]
    fn invalid_declaration_dropped() {
        // 缺分号/非法值：parse 后仅保留合法声明
        let div = styled_div("color: red; ;; junk");
        assert_eq!(div.style_property("color"), Some("red".to_string()));
    }
}

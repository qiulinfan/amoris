//! The lint's rules that hold anywhere in a module (script-sandbox.md 3): async code, decorators,
//! static private members and hooks among a class's statics, removed globals, assignments to the
//! global object, `RegExp.prototype.compile` and imports of the private module.

use oxc_ast::ast::{
    ArrowFunctionExpression, AssignmentExpression, AwaitExpression, Class, ClassElement, Decorator,
    Expression, ForOfStatement, Function, IdentifierReference, ImportExpression, ImportMeta,
    MemberExpression, PropertyKey, StaticMemberExpression, UnaryExpression, UnaryOperator,
    UpdateExpression,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;
use oxc_syntax::scope::ScopeFlags;

use super::{Linter, REMOVED};

/// The rules that hold anywhere in a module.
impl<'a> Visit<'a> for Linter<'_, 'a> {
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if it.r#async {
            self.report(it.span, "lint.async", "async functions leave pending work after the tick; keep waiting state in a component".into());
        }
        walk::walk_function(self, it, flags);
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        if it.r#async {
            self.report(it.span, "lint.async", "async functions leave pending work after the tick; keep waiting state in a component".into());
        }
        walk::walk_arrow_function_expression(self, it);
    }

    fn visit_await_expression(&mut self, it: &AwaitExpression<'a>) {
        self.report(
            it.span,
            "lint.async",
            "await leaves pending work after the tick".into(),
        );
        walk::walk_await_expression(self, it);
    }

    fn visit_for_of_statement(&mut self, it: &ForOfStatement<'a>) {
        if it.r#await {
            self.report(
                it.span,
                "lint.async",
                "for await leaves pending work after the tick".into(),
            );
        }
        walk::walk_for_of_statement(self, it);
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        self.report(
            it.span,
            "lint.async",
            "dynamic import() is not available; import statically".into(),
        );
        walk::walk_import_expression(self, it);
    }

    fn visit_import_meta(&mut self, it: &ImportMeta) {
        self.report(
            it.span,
            "lint.async",
            "import.meta is not available in game scripts".into(),
        );
    }

    fn visit_decorator(&mut self, it: &Decorator<'a>) {
        self.report(
            it.span(),
            "lint.decorator",
            "decorators run at load time; they are not available".into(),
        );
        walk::walk_decorator(self, it);
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        for el in &it.body.body {
            let (stat, key, computed, span) = match el {
                ClassElement::PropertyDefinition(p) => (p.r#static, &p.key, p.computed, p.span),
                ClassElement::MethodDefinition(m) => (m.r#static, &m.key, m.computed, m.span),
                ClassElement::AccessorProperty(a) => (a.r#static, &a.key, a.computed, a.span),
                _ => continue,
            };
            if !stat {
                continue;
            }
            if matches!(key, PropertyKey::PrivateIdentifier(_)) {
                self.report(
                    span,
                    "lint.static_private",
                    "a static private member keeps state the freeze cannot reach; keep it in a \
                     component"
                        .into(),
                );
            } else {
                self.hook_key(key, computed, span);
            }
        }
        walk::walk_class(self, it);
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if it
            .name
            .starts_with(crate::compile::transpile::RESERVED_PREFIX)
        {
            self.report(
                it.span,
                "lint.reserved_name",
                super::reserved_message(&it.name),
            );
        }
        if REMOVED.contains(&it.name.as_str()) && self.symbol(it).is_none() {
            let hint =
                crate::error::hint_for("ReferenceError", &format!("{} is not defined", it.name))
                    .unwrap_or_else(|| format!("{} is not available in game scripts", it.name));
            self.report(it.span, "lint.removed_global", hint);
        }
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        let target_root = it.left.as_simple_assignment_target().and_then(|t| match t {
            oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                Some(id.name.as_str())
            }
            other => other.as_member_expression().and_then(root_name),
        });
        if target_root == Some("globalThis") {
            self.report(
                it.span,
                "lint.removed_global",
                "the global object is frozen; keep state in a component".into(),
            );
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        if it.argument.as_member_expression().and_then(root_name) == Some("globalThis") {
            self.report(
                it.span,
                "lint.removed_global",
                "the global object is frozen; keep state in a component".into(),
            );
        }
        walk::walk_update_expression(self, it);
    }

    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        if it.operator == UnaryOperator::Delete
            && it
                .argument
                .get_inner_expression()
                .as_member_expression()
                .and_then(root_name)
                == Some("globalThis")
        {
            self.report(
                it.span,
                "lint.removed_global",
                "the global object is frozen".into(),
            );
        }
        walk::walk_unary_expression(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if it.property.name == "fromAsync" && self.is_global(&it.object, "Array") {
            self.report(
                it.span,
                "lint.removed_global",
                "Array.fromAsync is removed: it makes promises, whose jobs outlive the tick".into(),
            );
        }
        if it.property.name == "compile" {
            let regex = match it.object.get_inner_expression() {
                Expression::RegExpLiteral(_) => true,
                Expression::Identifier(id) => {
                    self.symbol(id).is_some_and(|s| self.regexes.contains(&s))
                }
                Expression::StaticMemberExpression(m) => {
                    m.property.name == "prototype" && self.is_global(&m.object, "RegExp")
                }
                _ => false,
            };
            if regex {
                self.report(
                    it.span,
                    "lint.removed_global",
                    "RegExp.prototype.compile is removed: it changes a regular expression in place"
                        .into(),
                );
            }
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_import_declaration(&mut self, it: &oxc_ast::ast::ImportDeclaration<'a>) {
        self.private_module(&it.source.value, it.span);
        walk::walk_import_declaration(self, it);
    }
}

/// The identifier a member chain starts from.
fn root_name<'b>(m: &'b MemberExpression<'_>) -> Option<&'b str> {
    let mut e = m.object();
    loop {
        match e.get_inner_expression() {
            Expression::Identifier(id) => return Some(id.name.as_str()),
            other => e = other.as_member_expression()?.object(),
        }
    }
}

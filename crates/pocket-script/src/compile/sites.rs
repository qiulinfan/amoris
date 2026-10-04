//! Where each system's `run` is written: `system({ name: "x", ..., run(ctx) {...} })` calls of
//! `pocket`'s builder with a literal name. `SystemInfo::defined_at` comes from here, and so does the
//! location of a failure no single line caused (a column write-back checked after the call).

use std::collections::BTreeMap;

use oxc_ast::ast::{
    CallExpression, Expression, ImportDeclarationSpecifier, ModuleExportName, ObjectPropertyKind,
    Program, PropertyKey, Statement,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::Semantic;
use oxc_syntax::symbol::SymbolId;

use crate::error::line_col;
use crate::resolve::PRELUDE;
use crate::source::SystemSite;

struct Finder<'s, 'a> {
    semantic: &'s Semantic<'a>,
    source: &'s str,
    /// `pocket` imports: the local symbol and the imported name (`*` for a namespace).
    imports: BTreeMap<SymbolId, String>,
    found: Vec<SystemSite>,
}

impl<'a> Finder<'_, 'a> {
    fn pocket_name(&self, e: &Expression<'a>) -> Option<String> {
        let symbol = |id: &oxc_ast::ast::IdentifierReference<'a>| {
            let r = id.reference_id.get()?;
            self.semantic.scoping().get_reference(r).symbol_id()
        };
        match e.get_inner_expression() {
            Expression::Identifier(id) => self
                .imports
                .get(&symbol(id)?)
                .filter(|n| *n != "*")
                .cloned(),
            Expression::StaticMemberExpression(m) => match m.object.get_inner_expression() {
                Expression::Identifier(id) => {
                    (self.imports.get(&symbol(id)?)? == "*").then(|| m.property.name.to_string())
                }
                _ => None,
            },
            _ => None,
        }
    }
}

impl<'a> Visit<'a> for Finder<'_, 'a> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let builder = self.pocket_name(&it.callee);
        if matches!(builder.as_deref(), Some("system" | "executor"))
            && let Some(Expression::ObjectExpression(o)) =
                it.arguments.first().and_then(|a| a.as_expression())
        {
            let mut name = None;
            let mut run = None;
            for p in &o.properties {
                let ObjectPropertyKind::ObjectProperty(p) = p else {
                    continue;
                };
                let key = match &p.key {
                    PropertyKey::StaticIdentifier(id) => id.name.as_str(),
                    _ => continue,
                };
                match key {
                    "name" => {
                        if let Expression::StringLiteral(s) = &p.value {
                            name = Some(s.value.to_string());
                        }
                    }
                    "run" => run = Some(p.span.start),
                    _ => {}
                }
            }
            if let (Some(name), Some(at)) = (name, run) {
                let (line, column) = line_col(self.source, at);
                self.found.push(SystemSite { name, line, column });
            }
        }
        walk::walk_call_expression(self, it);
    }
}

/// The systems a module writes with a literal name, and where their `run` is.
pub fn system_sites<'a>(
    program: &Program<'a>,
    semantic: &Semantic<'a>,
    source: &str,
) -> Vec<SystemSite> {
    let mut imports = BTreeMap::new();
    for s in &program.body {
        let Statement::ImportDeclaration(d) = s else {
            continue;
        };
        if d.source.value != PRELUDE {
            continue;
        }
        for spec in d.specifiers.iter().flatten() {
            let (local, name) = match spec {
                ImportDeclarationSpecifier::ImportSpecifier(i) => {
                    let name = match &i.imported {
                        ModuleExportName::IdentifierName(n) => n.name.to_string(),
                        ModuleExportName::IdentifierReference(n) => n.name.to_string(),
                        ModuleExportName::StringLiteral(n) => n.value.to_string(),
                    };
                    (&i.local, name)
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(i) => {
                    (&i.local, "default".into())
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(i) => (&i.local, "*".into()),
            };
            imports.insert(local.symbol_id(), name);
        }
    }
    let mut f = Finder {
        semantic,
        source,
        imports,
        found: Vec::new(),
    };
    f.visit_program(program);
    f.found
}

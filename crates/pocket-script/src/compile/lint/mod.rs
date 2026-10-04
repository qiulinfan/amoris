//! The stateless-script lint (docs/spec/script-sandbox.md 3): a pass over oxc's TypeScript AST and
//! scoping inside `compile`, so whatever compiles has passed it. Every rule is an error; there are
//! no suppression comments.

use std::collections::BTreeMap;

use oxc_ast::ast::{
    Argument, ArrayExpressionElement, BindingPattern, CallExpression, Class, ClassElement,
    Declaration, Expression, IdentifierReference, ImportDeclarationSpecifier, ModuleExportName,
    ObjectPropertyKind, Program, PropertyKey, PropertyKind, Statement, TSNamespaceDeclaration,
    TSNamespaceDeclarationBody, UnaryOperator, VariableDeclarationKind,
};
use oxc_ast_visit::Visit;
use oxc_semantic::Semantic;
use oxc_span::{GetSpan, Span};
use oxc_syntax::symbol::SymbolId;
use serde_json::json;

use crate::error::{ErrorPhase, ScriptError, line_col};
use crate::resolve::{HOST, PRELUDE};

mod anywhere;

/// Globals the sandbox removes (2.2) and names agents reach for that do not exist.
const REMOVED: &[&str] = &[
    "AsyncDisposableStack",
    "Date",
    "performance",
    "Promise",
    "queueMicrotask",
    "WeakRef",
    "FinalizationRegistry",
    "SharedArrayBuffer",
    "Atomics",
    "eval",
    "require",
    "process",
    "window",
    "document",
    "fetch",
    "setTimeout",
    "setInterval",
    "clearTimeout",
    "clearInterval",
];

/// The message of `lint.reserved_name`.
fn reserved_message(name: &str) -> String {
    format!(
        "'{name}': names starting with {} belong to the script host; rename it",
        super::transpile::RESERVED_PREFIX
    )
}

/// `pocket` exports tagged `@pure`: callable at load time.
const PURE_POCKET: &[&str] = &["game", "system", "executor", "component", "freeze"];
/// The `field` builders, all pure.
const PURE_FIELDS: &[&str] = &[
    "f64", "i32", "u32", "tick", "bool", "str", "entity", "vec2", "vec3", "vec4", "quat", "enum",
];
/// Keys through which a load-time conversion, iteration or `instanceof` would call project code.
const HOOKS: &[&str] = &["valueOf", "toString", "toLocaleString", "toJSON"];

struct Linter<'s, 'a> {
    file: &'s str,
    source: &'s str,
    semantic: &'s Semantic<'a>,
    prelude: bool,
    /// Imported bindings: the specifier and the imported name (`*` for a namespace).
    imports: BTreeMap<SymbolId, (String, String)>,
    /// Top-level consts bound to a regular expression literal.
    regexes: Vec<SymbolId>,
    found: Vec<(u32, &'static str, String)>,
}

impl<'a> Linter<'_, 'a> {
    fn report(&mut self, span: Span, code: &'static str, message: String) {
        self.found.push((span.start, code, message));
    }

    /// `lint.reserved_name`: no binding, at any level, with the harden epilogue's prefix (a
    /// top-level one would collide with its imports; references are refused in the visitor).
    fn reserved_bindings(&mut self) {
        let scoping = self.semantic.scoping();
        let found: Vec<(Span, String)> = scoping
            .symbol_ids()
            .filter(|&id| {
                scoping
                    .symbol_name(id)
                    .starts_with(super::transpile::RESERVED_PREFIX)
            })
            .map(|id| (scoping.symbol_span(id), scoping.symbol_name(id).to_owned()))
            .collect();
        for (span, name) in found {
            self.report(span, "lint.reserved_name", reserved_message(&name));
        }
    }

    /// The symbol an identifier reference resolves to; `None` for a global.
    fn symbol(&self, id: &IdentifierReference<'a>) -> Option<SymbolId> {
        let reference = id.reference_id.get()?;
        self.semantic.scoping().get_reference(reference).symbol_id()
    }

    fn is_global(&self, e: &Expression<'a>, name: &str) -> bool {
        matches!(e.get_inner_expression(), Expression::Identifier(id) if id.name == name && self.symbol(id).is_none())
    }

    /// The `pocket` export an expression names: `game`, or `field` for `field` itself.
    fn pocket_name(&self, e: &Expression<'a>) -> Option<String> {
        match e.get_inner_expression() {
            Expression::Identifier(id) => {
                let (spec, name) = self.imports.get(&self.symbol(id)?)?;
                (spec == PRELUDE && name != "*").then(|| name.clone())
            }
            Expression::StaticMemberExpression(m) => match m.object.get_inner_expression() {
                Expression::Identifier(id) => {
                    let (spec, name) = self.imports.get(&self.symbol(id)?)?;
                    (spec == PRELUDE && name == "*").then(|| m.property.name.to_string())
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether a load-time call may call `callee`: a pure `pocket` export or `field` builder, a
    /// `Math` function but `random`, a static function of `Number`, and a few of `String`,
    /// `Object` and `Array`.
    fn pure_callee(&self, callee: &Expression<'a>) -> bool {
        if let Some(name) = self.pocket_name(callee) {
            return PURE_POCKET.contains(&name.as_str());
        }
        let Expression::StaticMemberExpression(m) = callee.get_inner_expression() else {
            return false;
        };
        let p = m.property.name.as_str();
        if self.pocket_name(&m.object).as_deref() == Some("field") {
            return PURE_FIELDS.contains(&p);
        }
        let global = |n| self.is_global(&m.object, n);
        (global("Math") && p != "random")
            || (global("Number")
                && matches!(
                    p,
                    "isFinite"
                        | "isInteger"
                        | "isNaN"
                        | "isSafeInteger"
                        | "parseFloat"
                        | "parseInt"
                ))
            || (global("String") && matches!(p, "fromCharCode" | "fromCodePoint"))
            || (global("Object") && matches!(p, "freeze" | "keys" | "values" | "entries"))
            || (global("Array") && matches!(p, "isArray" | "of"))
    }

    fn load_time_error(&mut self, span: Span, what: &str) {
        self.report(
            span,
            "lint.load_time_code",
            format!(
                "{what} runs while the module loads; module-level values may only be literals, \
                 functions, classes and pure builder calls (keep state in components)"
            ),
        );
    }

    /// Checks an expression that runs while the module evaluates.
    fn load_time(&mut self, e: &Expression<'a>) {
        match e.get_inner_expression() {
            Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BigIntLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::Identifier(_)
            | Expression::ThisExpression(_)
            | Expression::FunctionExpression(_)
            | Expression::ArrowFunctionExpression(_) => {}
            Expression::RegExpLiteral(r) => {
                use oxc_ast::ast::RegExpFlags;
                if r.regex.flags.intersects(RegExpFlags::G | RegExpFlags::Y) {
                    self.load_time_error(
                        r.span,
                        "a module-level regular expression with g or y (its lastIndex changes)",
                    );
                }
            }
            Expression::TemplateLiteral(t) => {
                for x in &t.expressions {
                    self.load_time(x);
                }
            }
            Expression::StaticMemberExpression(m) => {
                if m.property.name == "prototype" {
                    self.load_time_error(m.span, "reading .prototype");
                }
                self.load_time(&m.object);
            }
            Expression::ComputedMemberExpression(m) => {
                if matches!(&m.expression, Expression::StringLiteral(s) if s.value == "prototype") {
                    self.load_time_error(m.span, "reading .prototype");
                }
                self.load_time(&m.object);
                self.load_time(&m.expression);
            }
            Expression::PrivateFieldExpression(m) => self.load_time(&m.object),
            Expression::ChainExpression(c) => {
                use oxc_ast::ast::ChainElement;
                match &c.expression {
                    ChainElement::CallExpression(call) => self.load_time_call(call),
                    ChainElement::TSNonNullExpression(n) => self.load_time(&n.expression),
                    other => {
                        if let Some(m) = other.as_member_expression() {
                            self.load_time(m.object());
                        }
                    }
                }
            }
            Expression::ObjectExpression(o) => {
                for p in &o.properties {
                    match p {
                        ObjectPropertyKind::SpreadProperty(s) => self.load_time(&s.argument),
                        ObjectPropertyKind::ObjectProperty(p) => {
                            if p.kind != PropertyKind::Init {
                                self.load_time_error(
                                    p.span,
                                    "a getter or setter in a module-level object",
                                );
                            }
                            self.hook_key(&p.key, p.computed, p.span);
                            if !p.method {
                                self.load_time(&p.value);
                            }
                        }
                    }
                }
            }
            Expression::ArrayExpression(a) => {
                for el in &a.elements {
                    match el {
                        ArrayExpressionElement::SpreadElement(s) => self.load_time(&s.argument),
                        ArrayExpressionElement::Elision(_) => {}
                        other => {
                            if let Some(x) = other.as_expression() {
                                self.load_time(x);
                            }
                        }
                    }
                }
            }
            Expression::ClassExpression(c) => self.load_time_class(c),
            Expression::UnaryExpression(u) => {
                if u.operator == UnaryOperator::Delete {
                    self.load_time_error(u.span, "delete");
                }
                self.load_time(&u.argument);
            }
            Expression::BinaryExpression(b) => {
                self.load_time(&b.left);
                self.load_time(&b.right);
            }
            Expression::LogicalExpression(b) => {
                self.load_time(&b.left);
                self.load_time(&b.right);
            }
            Expression::ConditionalExpression(c) => {
                self.load_time(&c.test);
                self.load_time(&c.consequent);
                self.load_time(&c.alternate);
            }
            Expression::SequenceExpression(s) => {
                for x in &s.expressions {
                    self.load_time(x);
                }
            }
            Expression::PrivateInExpression(p) => self.load_time(&p.right),
            Expression::CallExpression(call) => self.load_time_call(call),
            Expression::NewExpression(n) => self.load_time_error(n.span, "`new`"),
            Expression::TaggedTemplateExpression(t) => {
                self.load_time_error(t.span, "a tagged template");
            }
            other => self.load_time_error(other.span(), "this expression"),
        }
    }

    fn load_time_call(&mut self, call: &CallExpression<'a>) {
        if !self.pure_callee(&call.callee) {
            self.load_time_error(
                call.span,
                "a call of a function other than pocket's builders",
            );
            return;
        }
        for a in &call.arguments {
            match a {
                Argument::SpreadElement(s) => self.load_time(&s.argument),
                other => {
                    if let Some(x) = other.as_expression() {
                        self.load_time(x);
                    }
                }
            }
        }
    }

    /// A key that would hand a load-time conversion to project code.
    fn hook_key(&mut self, key: &PropertyKey<'a>, computed: bool, span: Span) {
        if computed {
            self.report(
                span,
                "lint.load_time_hook",
                "a computed key in a module-level object or among a class's static members could \
                 name a hook ([Symbol.iterator], [Symbol.toPrimitive]) that runs project code at \
                 load time"
                    .into(),
            );
            return;
        }
        let name = match key {
            PropertyKey::StaticIdentifier(id) => Some(id.name.as_str()),
            PropertyKey::StringLiteral(s) => Some(s.value.as_str()),
            _ => None,
        };
        if let Some(n) = name.filter(|n| HOOKS.contains(n)) {
            self.report(
                span,
                "lint.load_time_hook",
                format!("'{n}' in a module-level object or a class's statics runs project code in conversions at load time"),
            );
        }
    }

    /// A class evaluated at load time: its heritage, computed keys and static initializers run.
    fn load_time_class(&mut self, c: &Class<'a>) {
        if let Some(h) = &c.heritage {
            self.load_time(&h.expression);
        }
        for el in &c.body.body {
            match el {
                ClassElement::StaticBlock(b) => {
                    self.load_time_error(b.span, "a class static block")
                }
                ClassElement::PropertyDefinition(p) => {
                    if p.computed
                        && let Some(k) = p.key.as_expression()
                    {
                        self.load_time(k);
                    }
                    if p.r#static
                        && let Some(v) = &p.value
                    {
                        self.load_time(v);
                    }
                }
                ClassElement::MethodDefinition(m) => {
                    use oxc_ast::ast::MethodDefinitionKind;
                    if m.computed
                        && let Some(k) = m.key.as_expression()
                    {
                        self.load_time(k);
                    }
                    if m.r#static
                        && matches!(
                            m.kind,
                            MethodDefinitionKind::Get | MethodDefinitionKind::Set
                        )
                    {
                        self.load_time_error(m.span, "a static accessor");
                    }
                }
                ClassElement::AccessorProperty(a) => {
                    if a.r#static {
                        self.load_time_error(a.span, "a static accessor");
                    }
                }
                ClassElement::TSIndexSignature(_) => {}
            }
        }
    }

    /// Defaults and computed keys of a top-level destructuring run at load time.
    fn load_time_pattern(&mut self, p: &BindingPattern<'a>) {
        match p {
            BindingPattern::BindingIdentifier(_) => {}
            BindingPattern::AssignmentPattern(a) => {
                self.load_time(&a.right);
                self.load_time_pattern(&a.left);
            }
            BindingPattern::ObjectPattern(o) => {
                for prop in &o.properties {
                    if prop.computed
                        && let Some(k) = prop.key.as_expression()
                    {
                        self.load_time(k);
                    }
                    self.load_time_pattern(&prop.value);
                }
                if let Some(r) = &o.rest {
                    self.load_time_pattern(&r.argument);
                }
            }
            BindingPattern::ArrayPattern(a) => {
                for el in a.elements.iter().flatten() {
                    self.load_time_pattern(el);
                }
                if let Some(r) = &a.rest {
                    self.load_time_pattern(&r.argument);
                }
            }
        }
    }

    fn module_statement(&mut self, span: Span, what: &str) {
        self.report(
            span,
            "lint.module_statement",
            format!(
                "{what} at the top level runs while the module loads; top-level statements are \
                 imports, exports, const, function, class, enum and type declarations"
            ),
        );
    }

    fn declaration(&mut self, d: &Declaration<'a>) {
        match d {
            Declaration::VariableDeclaration(v) => {
                if v.declare {
                    return;
                }
                if v.kind != VariableDeclarationKind::Const {
                    for decl in &v.declarations {
                        let names: Vec<String> = decl
                            .id
                            .get_binding_identifiers()
                            .iter()
                            .map(|b| b.name.to_string())
                            .collect();
                        self.report(
                            decl.span,
                            "lint.module_let",
                            format!(
                                "'{}' is a module-level variable, state outside the world: use \
                                 const for a constant, or keep the state in a component",
                                names.join(", ")
                            ),
                        );
                    }
                }
                for decl in &v.declarations {
                    if decl.id.get_binding_identifier().is_none() {
                        self.load_time_pattern(&decl.id);
                    }
                    if let Some(init) = &decl.init {
                        if let (Expression::RegExpLiteral(_), Some(b)) = (
                            init.get_inner_expression(),
                            decl.id.get_binding_identifier(),
                        ) {
                            self.regexes.push(b.symbol_id());
                        }
                        self.load_time(init);
                    }
                }
            }
            Declaration::FunctionDeclaration(_)
            | Declaration::TSTypeAliasDeclaration(_)
            | Declaration::TSInterfaceDeclaration(_)
            | Declaration::TSExternalModuleDeclaration(_)
            | Declaration::TSGlobalDeclaration(_) => {}
            Declaration::ClassDeclaration(c) => {
                if !c.declare {
                    self.load_time_class(c);
                }
            }
            Declaration::TSEnumDeclaration(e) => {
                if !e.declare {
                    for m in &e.body.members {
                        if let Some(init) = &m.initializer {
                            self.load_time(init);
                        }
                    }
                }
            }
            Declaration::TSNamespaceDeclaration(n) => {
                if !n.declare && namespace_has_values(n) {
                    self.report(
                        n.span,
                        "lint.namespace",
                        "a namespace with values compiles to code that runs at load time; use a \
                         module instead"
                            .into(),
                    );
                }
            }
            Declaration::TSImportEqualsDeclaration(i) => {
                self.module_statement(i.span, "import = require(...)");
            }
        }
    }

    fn top_level(&mut self, program: &Program<'a>) {
        for s in &program.body {
            match s {
                Statement::ImportDeclaration(_) | Statement::ExportNamedDeclaration(_) => {}
                Statement::ExportAllDeclaration(d) => self.private_module(&d.source.value, d.span),
                Statement::ExportFromDeclaration(d) => self.private_module(&d.source.value, d.span),
                Statement::ExportDeclaration(d) => self.declaration(&d.declaration),
                Statement::ExportDefaultDeclaration(d) => {
                    use oxc_ast::ast::ExportDefaultDeclarationKind as K;
                    match &d.declaration {
                        K::FunctionDeclaration(_) | K::TSInterfaceDeclaration(_) => {}
                        K::ClassDeclaration(c) => self.load_time_class(c),
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.load_time(e);
                            }
                        }
                    }
                }
                Statement::TSExportAssignment(a) => self.module_statement(a.span, "export ="),
                Statement::TSNamespaceExportDeclaration(_) | Statement::EmptyStatement(_) => {}
                other => match other.as_declaration() {
                    Some(d) => self.declaration(d),
                    None => self.module_statement(other.span(), "this statement"),
                },
            }
        }
    }

    fn private_module(&mut self, specifier: &str, span: Span) {
        if specifier == HOST && !self.prelude {
            self.report(
                span,
                "lint.private_module",
                "pocket:host is the engine's private module; import from \"pocket\"".into(),
            );
        }
    }

    /// Every write to a top-level binding, anywhere in the module.
    fn reassignments(&mut self) {
        let scoping = self.semantic.scoping();
        let mut found = Vec::new();
        for (_, &symbol) in scoping.get_bindings(scoping.root_scope_id()).iter() {
            for r in scoping.get_resolved_references(symbol) {
                if r.is_write() {
                    let span = self.semantic.nodes().get_node(r.node_id()).span();
                    found.push((span, scoping.symbol_name(symbol).to_owned()));
                }
            }
        }
        for (span, name) in found {
            self.report(
                span,
                "lint.module_reassign",
                format!("this assigns the module-level binding '{name}', state outside the world: keep it in a component"),
            );
        }
    }
}

fn namespace_has_values(n: &TSNamespaceDeclaration<'_>) -> bool {
    match &n.body {
        TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => namespace_has_values(inner),
        TSNamespaceDeclarationBody::TSModuleBlock(b) => b.body.iter().any(|s| match s {
            Statement::TSTypeAliasDeclaration(_) | Statement::TSInterfaceDeclaration(_) => false,
            Statement::TSNamespaceDeclaration(inner) => namespace_has_values(inner),
            Statement::ExportDeclaration(e) => !matches!(
                e.declaration,
                Declaration::TSTypeAliasDeclaration(_) | Declaration::TSInterfaceDeclaration(_)
            ),
            _ => true,
        }),
    }
}

/// Lints one module; an empty result means every rule holds.
pub fn lint<'a>(
    file: &str,
    source: &str,
    program: &Program<'a>,
    semantic: &Semantic<'a>,
    prelude: bool,
) -> Vec<ScriptError> {
    let mut imports = BTreeMap::new();
    for s in &program.body {
        let Statement::ImportDeclaration(d) = s else {
            continue;
        };
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
            imports.insert(local.symbol_id(), (d.source.value.to_string(), name));
        }
    }
    let mut l = Linter {
        file,
        source,
        semantic,
        prelude,
        imports,
        regexes: Vec::new(),
        found: Vec::new(),
    };
    l.top_level(program);
    l.reassignments();
    l.reserved_bindings();
    l.visit_program(program);
    let mut found = std::mem::take(&mut l.found);
    found.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    found.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    found
        .into_iter()
        .map(|(start, code, message)| {
            let (line, column) = line_col(l.source, start);
            ScriptError::new(code, message, ErrorPhase::Lint)
                .at(l.file, line, column)
                .with("rule", json!(code))
        })
        .collect()
}

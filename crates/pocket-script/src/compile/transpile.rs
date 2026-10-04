//! One module through oxc (docs/spec/script-host.md 8.2): parse as a TypeScript module, build its
//! scoping, lint it, strip the types with `**` lowered to `Math.pow`, print JavaScript with a
//! source map, and append the harden epilogue (script-sandbox.md 2.4).

use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::Statement;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_syntax::symbol::SymbolFlags;
use oxc_transformer::{TransformOptions, Transformer};
use serde_json::json;

use crate::error::{ErrorPhase, ScriptError, line_col};
use crate::resolve::{HOST, PRELUDE};

/// The prefix of the names the harden epilogue binds in every module (`__pocket_harden`,
/// `__pocket_self`): `lint.reserved_name` refuses project bindings and references that use it.
pub const RESERVED_PREFIX: &str = "__pocket_";

/// One module, transpiled.
pub struct Transpiled {
    pub js: String,
    pub map: String,
    /// Every import and re-export specifier, in source order.
    pub imports: Vec<String>,
    /// Where the module's systems' `run`s are.
    pub systems: Vec<crate::source::SystemSite>,
}

/// How a module is treated.
#[derive(Clone, Copy, Debug, Default)]
pub struct ModuleOptions {
    /// The stateless-script lint is off (only the check's `module-state` negative control).
    pub lint_off: bool,
    /// The prelude: it may import `pocket:host`.
    pub prelude: bool,
}

fn diagnostic(
    code: &str,
    phase: ErrorPhase,
    file: &str,
    source: &str,
    d: &OxcDiagnostic,
) -> ScriptError {
    let offset = d.labels.first().map_or(0, |l| l.offset());
    let (line, column) = line_col(source, offset);
    ScriptError::new(code, d.message.to_string(), phase).at(file, line, column)
}

/// Transpiles module `file` (its module path, or the prelude's name).
pub fn transpile(
    file: &str,
    source: &str,
    options: ModuleOptions,
) -> Result<Transpiled, Vec<ScriptError>> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts().with_module(true)).parse();
    let syntax: Vec<ScriptError> = parsed
        .diagnostics
        .errors()
        .map(|d| diagnostic("script.syntax", ErrorPhase::Compile, file, source, d))
        .collect();
    if !syntax.is_empty() || parsed.fatal_error {
        return Err(syntax);
    }
    let mut program = parsed.program;
    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .with_build_nodes(true)
        .with_enum_eval(true)
        .build(&program);
    let errors: Vec<ScriptError> = semantic
        .diagnostics
        .errors()
        .map(|d| diagnostic("script.syntax", ErrorPhase::Compile, file, source, d))
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    let semantic = semantic.semantic;
    if !options.lint_off {
        let problems = super::lint::lint(file, source, &program, &semantic, options.prelude);
        if !problems.is_empty() {
            return Err(problems);
        }
    }
    let imports = imports(&program);
    let systems = super::sites::system_sites(&program, &semantic, source);
    let bindings = root_bindings(&semantic, source);
    let scoping = semantic.into_scoping();
    let mut transform = TransformOptions::default();
    transform.env.es2016.exponentiation_operator = true;
    let transformed = Transformer::new(&allocator, Path::new(file), &transform)
        .build_with_scoping(scoping, &mut program);
    let errors: Vec<ScriptError> = transformed
        .diagnostics
        .errors()
        .map(|d| diagnostic("script.syntax", ErrorPhase::Compile, file, source, d))
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    let out = Codegen::new()
        .with_options(CodegenOptions {
            source_map_path: Some(PathBuf::from(file)),
            ..CodegenOptions::default()
        })
        .with_scoping(Some(transformed.scoping))
        .build(&program);
    let map = out.map.map(|m| m.to_json_string()).unwrap_or_default();
    let mut js = out.code;
    js.push_str(&epilogue(file, &bindings));
    Ok(Transpiled {
        js,
        map,
        imports,
        systems,
    })
}

/// The specifiers a module imports or re-exports from.
fn imports(program: &oxc_ast::ast::Program<'_>) -> Vec<String> {
    let mut out = Vec::new();
    for s in &program.body {
        let source = match s {
            Statement::ImportDeclaration(d) => Some(&d.source),
            Statement::ExportAllDeclaration(d) => Some(&d.source),
            Statement::ExportFromDeclaration(d) => Some(&d.source),
            _ => None,
        };
        if let Some(src) = source {
            out.push(src.value.to_string());
        }
    }
    out
}

/// The module's own top-level value bindings (const, function, class and enum) with their lines,
/// in declaration order: what the epilogue hands to `harden`. Imports are hardened by their own
/// module.
fn root_bindings(semantic: &oxc_semantic::Semantic<'_>, source: &str) -> Vec<(String, u32)> {
    let scoping = semantic.scoping();
    let mut found: Vec<(u32, String)> = scoping
        .get_bindings(scoping.root_scope_id())
        .iter()
        .filter_map(|(_, &symbol)| {
            let flags = scoping.symbol_flags(symbol);
            let value = flags.intersects(
                SymbolFlags::Variable
                    | SymbolFlags::Function
                    | SymbolFlags::Class
                    | SymbolFlags::Enum,
            );
            let excluded = flags
                .intersects(SymbolFlags::Import | SymbolFlags::TypeImport | SymbolFlags::Ambient);
            (value && !excluded).then(|| {
                (
                    scoping.symbol_span(symbol).start,
                    scoping.symbol_name(symbol).to_owned(),
                )
            })
        })
        .collect();
    found.sort();
    found
        .into_iter()
        .map(|(start, n)| (n, line_col(source, start).0))
        .collect()
}

/// The harden epilogue: after the module's body, every top-level binding and its namespace (the
/// `default` export included, through a self-import) go to the private native `harden`, which
/// deep-freezes them and refuses state freezing cannot fix (`script.module_state`). Its own names
/// start with `RESERVED_PREFIX`, which the lint keeps from project code.
pub fn epilogue(file: &str, bindings: &[(String, u32)]) -> String {
    let own = if file == PRELUDE {
        PRELUDE.to_owned()
    } else {
        format!("./{}", file.rsplit('/').next().unwrap_or(file))
    };
    // Computed keys: `{ __proto__: __proto__ }` would set the object's prototype instead of adding
    // a key, and leave a top-level binding named `__proto__` unhardened.
    let values = bindings
        .iter()
        .map(|(b, _)| format!("[{}]: {b}", json!(b)))
        .collect::<Vec<_>>()
        .join(", ");
    let lines = bindings
        .iter()
        .map(|(b, l)| format!("[{}]: {l}", json!(b)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\nimport {{ harden as __pocket_harden }} from \"{HOST}\";\nimport * as __pocket_self \
         from {own};\n__pocket_harden({file}, __pocket_self, {{ {values} }}, {{ {lines} }});\n",
        own = json!(own),
        file = json!(file),
    )
}

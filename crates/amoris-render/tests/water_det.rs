//! The deterministic water kernels (water.md 66). The numerical kernels the authoritative water and
//! every visual replica run are generated from their WGSL sources by this test: naga parses each
//! composed module, every f32 addition, subtraction, multiplication and negation is rounded once
//! and passed through a guard no compiler can see through (an XOR with a zero read at run time), so
//! none is fused with another or reordered, and division, square roots and the transcendentals call
//! water_det.wgsl's functions, built from those operations. A guard also turns values under 2^-100
//! (subnormals among them) and negative zero into zero, which WGSL lets each device treat its own
//! way. `shaders/det/` holds what the pipelines load; `AMORIS_BLESS=1 cargo test -p amoris-render
//! --test water_det` rewrites it after a kernel changes.

use std::collections::HashMap;

use naga::valid::{Capabilities, FunctionInfo, ModuleInfo, ValidationFlags, Validator};
use naga::{
    AddressSpace, Arena, BinaryOperator, Block, Expression, Function, GlobalVariable, Handle,
    Literal, MathFunction, Module, Range, Scalar, ScalarKind, Span, Statement, Type, TypeInner,
    UnaryOperator, UniqueArena, VectorSize,
};

/// Each generated module: its name, the sources composed into it, and an expression of its
/// uniforms that is zero whenever it runs but that no compiler can know (water.md 66.2).
const MODULES: &[(&str, &[&str], &str)] = &[
    (
        "water_compute",
        &["water_compute.wgsl", "water_depth_first_order.wgsl"],
        "p.size.x >> 16u",
    ),
    (
        "water_maccormack",
        &["water_compute.wgsl", "water_maccormack.wgsl"],
        "p.size.x >> 16u",
    ),
    (
        "water_boundaries",
        &["water_compute.wgsl", "water_boundaries.wgsl"],
        "p.size.x >> 16u",
    ),
    ("water_nh", &["water_nh.wgsl"], "p.size.x >> 16u"),
    (
        "water_occupancy",
        &["water_occupancy.wgsl"],
        "p.size.x >> 16u",
    ),
    ("water_waves", &["water_waves.wgsl"], "p.size.x >> 16u"),
    ("water_fft", &["water_fft.wgsl"], "f.width >> 16u"),
    (
        "water_reactions",
        &["water_point.wgsl", "water_reactions.wgsl"],
        "p.shape.x >> 16u",
    ),
    ("water_edits", &["water_edits.wgsl"], "p.count >> 30u"),
    ("water_summary", &["water_summary.wgsl"], "p.width >> 16u"),
    (
        "water_samples",
        &["water_point.wgsl", "water_samples.wgsl"],
        "p.size.x >> 16u",
    ),
];

/// Exponents under this (2^-100) are flushed to zero by every guard.
const TINY: u32 = 27 << 23;
const EXPONENT: u32 = 0x7f80_0000;

fn shaders() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders")
}

fn validate(module: &Module) -> Result<ModuleInfo, String> {
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(module)
        .map_err(|e| format!("{e:?}"))
}

fn is_float(inner: &TypeInner) -> Option<Option<VectorSize>> {
    match *inner {
        TypeInner::Scalar(Scalar {
            kind: ScalarKind::Float,
            width: 4,
        }) => Some(None),
        TypeInner::Vector {
            size,
            scalar:
                Scalar {
                    kind: ScalarKind::Float,
                    width: 4,
                },
        } => Some(Some(size)),
        _ => None,
    }
}

/// The deterministic form of one composed WGSL module.
pub fn determinize(source: &str, zero: &str) -> Result<String, String> {
    let prelude = std::fs::read_to_string(shaders().join("water_det.wgsl")).unwrap();
    let source = format!("{prelude}\nfn det_zero() -> u32 {{ return {zero}; }}\n{source}");
    let mut module =
        naga::front::wgsl::parse_str(&source).map_err(|e| e.emit_to_string(&source))?;
    let info = validate(&module)?;
    let u32_ty = module.types.insert(
        Type {
            name: None,
            inner: TypeInner::Scalar(Scalar::U32),
        },
        Span::UNDEFINED,
    );
    let det_z = module.global_variables.append(
        GlobalVariable {
            name: Some("det_z".into()),
            space: AddressSpace::Private,
            binding: None,
            ty: u32_ty,
            init: None,
            memory_decorations: naga::MemoryDecorations::empty(),
        },
        Span::UNDEFINED,
    );
    let functions: HashMap<String, Handle<Function>> = module
        .functions
        .iter()
        .filter_map(|(h, f)| Some((f.name.clone()?, h)))
        .collect();
    let handles: Vec<Handle<Function>> = module.functions.iter().map(|(h, _)| h).collect();
    for h in handles {
        let name = module.functions[h].name.clone().unwrap_or_default();
        let rebuilt = Rewrite::new(
            &module.types,
            &info[h],
            &functions,
            det_z,
            name.starts_with("det_"),
        )
        .function(&module.functions[h])
        .map_err(|e| format!("{name}: {e}"))?;
        module.functions[h] = rebuilt;
    }
    for i in 0..module.entry_points.len() {
        let name = module.entry_points[i].name.clone();
        let mut rebuilt = Rewrite::new(
            &module.types,
            info.get_entry_point(i),
            &functions,
            det_z,
            false,
        )
        .function(&module.entry_points[i].function)
        .map_err(|e| format!("{name}: {e}"))?;
        // det_z = det_zero(); before anything else.
        let zero = functions["det_zero"];
        let result = rebuilt
            .expressions
            .append(Expression::CallResult(zero), Span::UNDEFINED);
        let pointer = rebuilt
            .expressions
            .append(Expression::GlobalVariable(det_z), Span::UNDEFINED);
        let mut body = Block::new();
        body.push(
            Statement::Call {
                function: zero,
                arguments: vec![],
                result: Some(result),
            },
            Span::UNDEFINED,
        );
        body.push(
            Statement::Store {
                pointer,
                value: result,
            },
            Span::UNDEFINED,
        );
        body.append(&mut rebuilt.body);
        rebuilt.body = body;
        module.entry_points[i].function = rebuilt;
    }
    naga::compact::compact(&mut module, naga::compact::KeepUnused::No);
    let info = validate(&module)?;
    let text =
        naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
            .map_err(|e| e.to_string())?;
    // What the pipelines load must parse and validate again.
    let again = naga::front::wgsl::parse_str(&text).map_err(|e| e.emit_to_string(&text))?;
    validate(&again)?;
    Ok(text)
}

/// One function rebuilt: its expressions in a new arena, in statement order, each f32 operation
/// followed by its guard and each inexact one replaced by a call.
struct Rewrite<'a> {
    types: &'a UniqueArena<Type>,
    info: &'a FunctionInfo,
    functions: &'a HashMap<String, Handle<Function>>,
    det_z: Handle<GlobalVariable>,
    /// Inside water_det.wgsl's own functions, which build the inexact operations: no calls.
    prelude: bool,
    old: Option<&'a Function>,
    new: Arena<Expression>,
    map: Vec<Option<Handle<Expression>>>,
    /// The guard's operands: the exponent mask, the smallest kept exponent, zero, det_z's value.
    consts: Option<[Handle<Expression>; 4]>,
    /// Where the expressions not yet covered by an Emit start.
    start: usize,
}

impl<'a> Rewrite<'a> {
    fn new(
        types: &'a UniqueArena<Type>,
        info: &'a FunctionInfo,
        functions: &'a HashMap<String, Handle<Function>>,
        det_z: Handle<GlobalVariable>,
        prelude: bool,
    ) -> Self {
        Self {
            types,
            info,
            functions,
            det_z,
            prelude,
            old: None,
            new: Arena::new(),
            map: Vec::new(),
            consts: None,
            start: 0,
        }
    }

    fn function(mut self, f: &'a Function) -> Result<Function, String> {
        self.old = Some(f);
        self.map = vec![None; f.expressions.len()];
        // Expressions no Emit covers: the pre-emitted ones (literals, arguments, variables) and
        // constants, copied first; the results of statements come with their statements.
        let mut emitted = vec![false; f.expressions.len()];
        mark_emitted(&f.body, &mut emitted);
        for (h, e) in f.expressions.iter() {
            let result = matches!(
                e,
                Expression::CallResult(_)
                    | Expression::AtomicResult { .. }
                    | Expression::WorkGroupUniformLoadResult { .. }
            );
            if !emitted[h.index()] && !result {
                let copied = self.copy(e)?;
                let span = f.expressions.get_span(h);
                let new = self.new.append(canonical(copied), span);
                self.map[h.index()] = Some(new);
            }
        }
        let mask = self.literal(Literal::U32(EXPONENT));
        let tiny = self.literal(Literal::U32(TINY));
        let zero = self.literal(Literal::U32(0));
        let z = self
            .new
            .append(Expression::GlobalVariable(self.det_z), Span::UNDEFINED);
        // det_z, read once where the function starts: its entry point has set it by then.
        self.start = self.new.len();
        let zv = self.push(Expression::Load { pointer: z });
        let mut body = Block::new();
        self.flush(&mut body);
        self.consts = Some([mask, tiny, zero, zv]);
        body.append(&mut self.block(&f.body)?);
        let mut out = f.clone();
        out.body = body;
        for (_, local) in out.local_variables.iter_mut() {
            if let Some(init) = local.init {
                local.init = Some(self.get(init)?);
            }
        }
        out.named_expressions = f
            .named_expressions
            .iter()
            .filter_map(|(h, name)| Some((self.map[h.index()]?, name.clone())))
            .collect();
        out.expressions = std::mem::take(&mut self.new);
        Ok(out)
    }

    fn literal(&mut self, l: Literal) -> Handle<Expression> {
        self.new.append(Expression::Literal(l), Span::UNDEFINED)
    }

    fn get(&self, h: Handle<Expression>) -> Result<Handle<Expression>, String> {
        self.map[h.index()].ok_or_else(|| format!("expression {h:?} used before it is emitted"))
    }

    /// Closes the run of new expressions an Emit covers.
    fn flush(&mut self, out: &mut Block) {
        let end = self.new.len();
        if end > self.start {
            let range = Range::from_index_range(self.start as u32..end as u32, &self.new);
            out.push(Statement::Emit(range), Span::UNDEFINED);
        }
        self.start = end;
    }

    fn push(&mut self, e: Expression) -> Handle<Expression> {
        self.new.append(e, Span::UNDEFINED)
    }

    /// x rounded and kept apart: bitcast to u32, small exponents to zero, XOR with det_z, back.
    fn guard(&mut self, x: Handle<Expression>, size: Option<VectorSize>) -> Handle<Expression> {
        let [mut mask, mut tiny, mut zero, mut zv] = self.consts.unwrap();
        if let Some(size) = size {
            mask = self.push(Expression::Splat { size, value: mask });
            tiny = self.push(Expression::Splat { size, value: tiny });
            zero = self.push(Expression::Splat { size, value: zero });
            zv = self.push(Expression::Splat { size, value: zv });
        }
        let bits = self.push(Expression::As {
            expr: x,
            kind: ScalarKind::Uint,
            convert: None,
        });
        let exponent = self.push(Expression::Binary {
            op: BinaryOperator::And,
            left: bits,
            right: mask,
        });
        let small = self.push(Expression::Binary {
            op: BinaryOperator::Less,
            left: exponent,
            right: tiny,
        });
        let kept = self.push(Expression::Select {
            condition: small,
            accept: zero,
            reject: bits,
        });
        let hidden = self.push(Expression::Binary {
            op: BinaryOperator::ExclusiveOr,
            left: kept,
            right: zv,
        });
        self.push(Expression::As {
            expr: hidden,
            kind: ScalarKind::Float,
            convert: None,
        })
    }

    /// A call of one of water_det.wgsl's functions, in place of an expression.
    fn call(
        &mut self,
        name: &str,
        arguments: Vec<Handle<Expression>>,
        out: &mut Block,
    ) -> Result<Handle<Expression>, String> {
        if self.prelude {
            return Err(format!(
                "water_det.wgsl's own functions must not call {name}"
            ));
        }
        let function = *self
            .functions
            .get(name)
            .ok_or_else(|| format!("water_det.wgsl has no {name}"))?;
        self.flush(out);
        let result = self.push(Expression::CallResult(function));
        out.push(
            Statement::Call {
                function,
                arguments,
                result: Some(result),
            },
            Span::UNDEFINED,
        );
        self.start = self.new.len();
        Ok(result)
    }

    fn sized(base: &str, size: Option<VectorSize>) -> String {
        match size {
            None => base.to_string(),
            Some(VectorSize::Bi) => format!("{base}2"),
            Some(VectorSize::Tri) => format!("{base}3"),
            Some(VectorSize::Quad) => format!("{base}4"),
        }
    }

    fn ty(&self, h: Handle<Expression>) -> &TypeInner {
        self.info[h].ty.inner_with(self.types)
    }

    /// An operand as the vector of `size` its operation needs (WGSL's scalar-vector forms).
    fn widen(
        &mut self,
        h: Handle<Expression>,
        size: Option<VectorSize>,
        old: Handle<Expression>,
    ) -> Handle<Expression> {
        match (size, self.ty(old)) {
            (Some(size), TypeInner::Scalar(_)) => self.push(Expression::Splat { size, value: h }),
            _ => h,
        }
    }

    /// Rewrites the old expression `h`, emitted where it was.
    fn expression(&mut self, h: Handle<Expression>, out: &mut Block) -> Result<(), String> {
        let old = self.old.unwrap();
        let e = &old.expressions[h];
        let float = is_float(self.ty(h));
        let new = match (e, float) {
            (
                &Expression::Binary {
                    op:
                        op @ (BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply),
                    left,
                    right,
                },
                Some(size),
            ) => {
                let (left, right) = (self.get(left)?, self.get(right)?);
                let x = self.push(Expression::Binary { op, left, right });
                self.guard(x, size)
            }
            (
                &Expression::Binary {
                    op: BinaryOperator::Divide,
                    left,
                    right,
                },
                Some(size),
            ) => {
                let (l, r) = (self.get(left)?, self.get(right)?);
                let l = self.widen(l, size, left);
                let r = self.widen(r, size, right);
                self.call(&Self::sized("det_div", size), vec![l, r], out)?
            }
            (
                &Expression::Binary {
                    op: BinaryOperator::Modulo,
                    ..
                },
                Some(_),
            ) => {
                return Err("f32 % has no deterministic form yet".into());
            }
            (
                &Expression::Unary {
                    op: UnaryOperator::Negate,
                    expr,
                },
                Some(size),
            ) => {
                let expr = self.get(expr)?;
                let x = self.push(Expression::Unary {
                    op: UnaryOperator::Negate,
                    expr,
                });
                self.guard(x, size)
            }
            (
                &Expression::Math {
                    fun,
                    arg,
                    arg1,
                    arg2,
                    arg3,
                },
                _,
            ) if is_float(self.ty(arg)).is_some() => {
                let size = is_float(self.ty(arg)).unwrap();
                let a = self.get(arg)?;
                let b = arg1.map(|x| self.get(x)).transpose()?;
                let c = arg2.map(|x| self.get(x)).transpose()?;
                let d = arg3.map(|x| self.get(x)).transpose()?;
                let scalar = |name: &str| -> Result<(), String> {
                    if size.is_some() {
                        Err(format!("vector {name} has no deterministic form yet"))
                    } else {
                        Ok(())
                    }
                };
                match fun {
                    // Exact on values with no subnormal and no negative zero.
                    MathFunction::Abs
                    | MathFunction::Min
                    | MathFunction::Max
                    | MathFunction::Clamp
                    | MathFunction::Saturate
                    | MathFunction::Sign
                    | MathFunction::Step => self.push(Expression::Math {
                        fun,
                        arg: a,
                        arg1: b,
                        arg2: c,
                        arg3: d,
                    }),
                    // Exact, but a negative argument can round to a negative zero.
                    MathFunction::Floor
                    | MathFunction::Ceil
                    | MathFunction::Round
                    | MathFunction::Trunc => {
                        let x = self.push(Expression::Math {
                            fun,
                            arg: a,
                            arg1: None,
                            arg2: None,
                            arg3: None,
                        });
                        self.guard(x, size)
                    }
                    MathFunction::Fract => {
                        let floor = self.push(Expression::Math {
                            fun: MathFunction::Floor,
                            arg: a,
                            arg1: None,
                            arg2: None,
                            arg3: None,
                        });
                        let x = self.push(Expression::Binary {
                            op: BinaryOperator::Subtract,
                            left: a,
                            right: floor,
                        });
                        self.guard(x, size)
                    }
                    MathFunction::Sqrt => {
                        scalar("sqrt")?;
                        self.call("det_sqrt", vec![a], out)?
                    }
                    MathFunction::InverseSqrt => {
                        scalar("inverseSqrt")?;
                        self.call("det_rsqrt", vec![a], out)?
                    }
                    MathFunction::Exp => {
                        scalar("exp")?;
                        self.call("det_exp", vec![a], out)?
                    }
                    MathFunction::Log => {
                        scalar("log")?;
                        self.call("det_log", vec![a], out)?
                    }
                    MathFunction::Pow => {
                        scalar("pow")?;
                        self.call("det_pow", vec![a, b.unwrap()], out)?
                    }
                    MathFunction::Sin => {
                        scalar("sin")?;
                        self.call("det_sin", vec![a], out)?
                    }
                    MathFunction::Cos => {
                        scalar("cos")?;
                        self.call("det_cos", vec![a], out)?
                    }
                    MathFunction::Tanh => {
                        scalar("tanh")?;
                        self.call("det_tanh", vec![a], out)?
                    }
                    MathFunction::Acosh => {
                        scalar("acosh")?;
                        self.call("det_acosh", vec![a], out)?
                    }
                    MathFunction::SmoothStep => {
                        scalar("smoothstep")?;
                        self.call("det_smoothstep", vec![a, b.unwrap(), c.unwrap()], out)?
                    }
                    MathFunction::Length => match size {
                        Some(VectorSize::Bi) => self.call("det_length2", vec![a], out)?,
                        Some(VectorSize::Tri) => self.call("det_length3", vec![a], out)?,
                        _ => {
                            let x = self.push(Expression::Math {
                                fun: MathFunction::Abs,
                                arg: a,
                                arg1: None,
                                arg2: None,
                                arg3: None,
                            });
                            if size.is_some() {
                                return Err("length of a vec4 has no deterministic form yet".into());
                            }
                            x
                        }
                    },
                    MathFunction::Dot => {
                        let name = Self::sized("det_dot", size);
                        self.call(&name, vec![a, b.unwrap()], out)?
                    }
                    MathFunction::Mix => {
                        let t = self.widen(c.unwrap(), size, arg2.unwrap());
                        self.call(&Self::sized("det_mix", size), vec![a, b.unwrap(), t], out)?
                    }
                    MathFunction::Fma => {
                        scalar("fma")?;
                        self.call("det_fma", vec![a, b.unwrap(), c.unwrap()], out)?
                    }
                    other => {
                        return Err(format!("{other:?} has no deterministic form yet"));
                    }
                }
            }
            _ => {
                let copied = self.copy(e)?;
                self.push(canonical(copied))
            }
        };
        self.map[h.index()] = Some(new);
        Ok(())
    }

    /// `e` with its operands' new handles.
    fn copy(&self, e: &Expression) -> Result<Expression, String> {
        use Expression as E;
        let g = |h: Handle<Expression>| self.get(h);
        Ok(match *e {
            E::Literal(_)
            | E::Constant(_)
            | E::Override(_)
            | E::ZeroValue(_)
            | E::FunctionArgument(_)
            | E::GlobalVariable(_)
            | E::LocalVariable(_)
            | E::CallResult(_)
            | E::AtomicResult { .. }
            | E::WorkGroupUniformLoadResult { .. } => e.clone(),
            E::Compose { ty, ref components } => E::Compose {
                ty,
                components: components.iter().map(|&c| g(c)).collect::<Result<_, _>>()?,
            },
            E::Access { base, index } => E::Access {
                base: g(base)?,
                index: g(index)?,
            },
            E::AccessIndex { base, index } => E::AccessIndex {
                base: g(base)?,
                index,
            },
            E::Splat { size, value } => E::Splat {
                size,
                value: g(value)?,
            },
            E::Swizzle {
                size,
                vector,
                pattern,
            } => E::Swizzle {
                size,
                vector: g(vector)?,
                pattern,
            },
            E::Load { pointer } => E::Load {
                pointer: g(pointer)?,
            },
            E::Unary { op, expr } => E::Unary { op, expr: g(expr)? },
            E::Binary { op, left, right } => E::Binary {
                op,
                left: g(left)?,
                right: g(right)?,
            },
            E::Select {
                condition,
                accept,
                reject,
            } => E::Select {
                condition: g(condition)?,
                accept: g(accept)?,
                reject: g(reject)?,
            },
            E::Relational { fun, argument } => E::Relational {
                fun,
                argument: g(argument)?,
            },
            E::Math {
                fun,
                arg,
                arg1,
                arg2,
                arg3,
            } => E::Math {
                fun,
                arg: g(arg)?,
                arg1: arg1.map(g).transpose()?,
                arg2: arg2.map(g).transpose()?,
                arg3: arg3.map(g).transpose()?,
            },
            E::As {
                expr,
                kind,
                convert,
            } => E::As {
                expr: g(expr)?,
                kind,
                convert,
            },
            E::ArrayLength(h) => E::ArrayLength(g(h)?),
            ref other => {
                return Err(format!(
                    "{other:?} is not supported in deterministic kernels"
                ));
            }
        })
    }

    fn block(&mut self, old: &Block) -> Result<Block, String> {
        let mut out = Block::new();
        for (s, &span) in old.span_iter() {
            match *s {
                Statement::Emit(ref range) => {
                    for h in range.clone() {
                        self.expression(h, &mut out)?;
                    }
                    self.flush(&mut out);
                }
                Statement::Block(ref b) => {
                    let b = self.block(b)?;
                    out.push(Statement::Block(b), span);
                }
                Statement::If {
                    condition,
                    ref accept,
                    ref reject,
                } => {
                    let condition = self.get(condition)?;
                    let accept = self.block(accept)?;
                    let reject = self.block(reject)?;
                    out.push(
                        Statement::If {
                            condition,
                            accept,
                            reject,
                        },
                        span,
                    );
                }
                Statement::Switch {
                    selector,
                    ref cases,
                } => {
                    let selector = self.get(selector)?;
                    let mut new_cases = Vec::with_capacity(cases.len());
                    for case in cases {
                        new_cases.push(naga::SwitchCase {
                            value: case.value,
                            body: self.block(&case.body)?,
                            fall_through: case.fall_through,
                        });
                    }
                    out.push(
                        Statement::Switch {
                            selector,
                            cases: new_cases,
                        },
                        span,
                    );
                }
                Statement::Loop {
                    ref body,
                    ref continuing,
                    break_if,
                } => {
                    let body = self.block(body)?;
                    let continuing = self.block(continuing)?;
                    let break_if = break_if.map(|h| self.get(h)).transpose()?;
                    out.push(
                        Statement::Loop {
                            body,
                            continuing,
                            break_if,
                        },
                        span,
                    );
                }
                Statement::Break
                | Statement::Continue
                | Statement::Kill
                | Statement::ControlBarrier(_)
                | Statement::MemoryBarrier(_) => out.push(s.clone(), span),
                Statement::Return { value } => {
                    let value = value.map(|h| self.get(h)).transpose()?;
                    out.push(Statement::Return { value }, span);
                }
                Statement::Store { pointer, value } => {
                    let (pointer, value) = (self.get(pointer)?, self.get(value)?);
                    out.push(Statement::Store { pointer, value }, span);
                }
                Statement::Atomic {
                    pointer,
                    ref fun,
                    value,
                    result,
                } => {
                    let fun = match *fun {
                        naga::AtomicFunction::Exchange {
                            compare: Some(compare),
                        } => naga::AtomicFunction::Exchange {
                            compare: Some(self.get(compare)?),
                        },
                        other => other,
                    };
                    let (pointer, value) = (self.get(pointer)?, self.get(value)?);
                    let result = match result {
                        Some(r) => Some(self.result(r)?),
                        None => None,
                    };
                    out.push(
                        Statement::Atomic {
                            pointer,
                            fun,
                            value,
                            result,
                        },
                        span,
                    );
                    self.start = self.new.len();
                }
                Statement::WorkGroupUniformLoad { pointer, result } => {
                    let pointer = self.get(pointer)?;
                    let result = self.result(result)?;
                    out.push(Statement::WorkGroupUniformLoad { pointer, result }, span);
                    self.start = self.new.len();
                }
                Statement::Call {
                    function,
                    ref arguments,
                    result,
                } => {
                    let arguments = arguments
                        .iter()
                        .map(|&a| self.get(a))
                        .collect::<Result<Vec<_>, _>>()?;
                    let result = match result {
                        Some(r) => Some(self.result(r)?),
                        None => None,
                    };
                    out.push(
                        Statement::Call {
                            function,
                            arguments,
                            result,
                        },
                        span,
                    );
                    self.start = self.new.len();
                }
                ref other => {
                    return Err(format!(
                        "{other:?} is not supported in deterministic kernels"
                    ));
                }
            }
        }
        Ok(out)
    }

    /// The new expression a statement's result is.
    fn result(&mut self, old: Handle<Expression>) -> Result<Handle<Expression>, String> {
        let e = self.old.unwrap().expressions[old].clone();
        let new = self.new.append(e, Span::UNDEFINED);
        self.map[old.index()] = Some(new);
        Ok(new)
    }
}

/// A literal as every device reads it: no negative zero or value under 2^-100.
fn canonical(e: Expression) -> Expression {
    match e {
        Expression::Literal(Literal::F32(x)) if x.to_bits() & EXPONENT < TINY => {
            Expression::Literal(Literal::F32(0.0))
        }
        e => e,
    }
}

fn mark_emitted(block: &Block, emitted: &mut [bool]) {
    for s in block.iter() {
        match *s {
            Statement::Emit(ref range) => {
                for h in range.clone() {
                    emitted[h.index()] = true;
                }
            }
            Statement::Block(ref b) => mark_emitted(b, emitted),
            Statement::If {
                ref accept,
                ref reject,
                ..
            } => {
                mark_emitted(accept, emitted);
                mark_emitted(reject, emitted);
            }
            Statement::Switch { ref cases, .. } => {
                for c in cases {
                    mark_emitted(&c.body, emitted);
                }
            }
            Statement::Loop {
                ref body,
                ref continuing,
                ..
            } => {
                mark_emitted(body, emitted);
                mark_emitted(continuing, emitted);
            }
            _ => {}
        }
    }
}

const HEADER: &str = "// Generated by tests/water_det.rs from the water kernels named below and water_det.wgsl\n// (AMORIS_BLESS=1 rewrites it; water.md 66). Edit the sources, not this file.\n";

#[test]
fn deterministic_kernels_are_current() {
    let dir = shaders().join("det");
    let bless = std::env::var_os("AMORIS_BLESS").is_some();
    let mut stale = Vec::new();
    for (name, sources, zero) in MODULES {
        let source = sources
            .iter()
            .map(|s| std::fs::read_to_string(shaders().join(s)).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let text = determinize(&source, zero).unwrap_or_else(|e| panic!("{name}: {e}"));
        let text = format!("{HEADER}// From: {}\n{text}", sources.join(", "));
        let path = dir.join(format!("{name}.wgsl"));
        if bless {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &text).unwrap();
        } else if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            stale.push(*name);
        }
    }
    assert!(
        stale.is_empty(),
        "{stale:?} differ from their sources: AMORIS_BLESS=1 cargo test -p amoris-render --test water_det"
    );
}

/// The kernels' arithmetic on the CPU (Rust's f32 operations round to nearest even and are never
/// fused): each rounded result through the guard, and water_det.wgsl's functions in its order.
// water_det.wgsl's literals as the GPU reads them, not the standard library's closer constants.
#[allow(clippy::approx_constant)]
mod cpu {
    pub fn g(x: f32) -> f32 {
        if x.to_bits() & super::EXPONENT < super::TINY {
            0.0
        } else {
            x
        }
    }
    fn rcp(b: f32) -> f32 {
        let m = b.abs();
        if m == 0.0 {
            return 0.0;
        }
        let mut x = f32::from_bits(0x7ef3_11c3u32.wrapping_sub(m.to_bits()));
        for _ in 0..4 {
            x = g(x * g(2.0 - g(m * x)));
        }
        if b < 0.0 { g(-x) } else { x }
    }
    pub fn div(a: f32, b: f32) -> f32 {
        g(a * rcp(b))
    }
    fn rsqrt(x: f32) -> f32 {
        let mut y = f32::from_bits(0x5f37_5a86u32.wrapping_sub(x.to_bits() >> 1));
        let h = g(0.5 * x);
        for _ in 0..3 {
            y = g(y * g(1.5 - g(g(h * y) * y)));
        }
        y
    }
    pub fn sqrt(x: f32) -> f32 {
        if x <= 0.0 {
            return 0.0;
        }
        let y = rsqrt(x);
        let s = g(x * y);
        g(s + g(g(0.5 * y) * g(x - g(s * s))))
    }
    /// Horner's rule as the kernels write it: c0 + x (c1 + x (c2 + ...)).
    fn horner(x: f32, c: &[f32]) -> f32 {
        let mut t = c[c.len() - 1];
        for &k in c[..c.len() - 1].iter().rev() {
            t = g(k + g(x * t));
        }
        t
    }
    pub fn exp(x: f32) -> f32 {
        if x < -69.0 {
            return 0.0;
        }
        let c = x.min(88.0);
        let n = g(g(c * 1.442_695).round_ties_even());
        let r = g(g(c - g(n * 0.693_145_75)) - g(n * 1.428_606_8e-6));
        let p = horner(
            r,
            &[
                1.0,
                1.0,
                0.5,
                0.166_666_67,
                0.041_666_668,
                0.008_333_334,
                0.001_388_888_9,
                0.000_198_412_7,
            ],
        );
        f32::from_bits(p.to_bits().wrapping_add(((n as i32) << 23) as u32))
    }
    pub fn log(x: f32) -> f32 {
        let u = x.to_bits();
        let mut e = (u >> 23) as i32 - 127;
        let mut m = f32::from_bits((u & 0x007f_ffff) | 0x3f80_0000);
        if m > 1.414_213_6 {
            m = g(m * 0.5);
            e += 1;
        }
        let s = div(g(m - 1.0), g(m + 1.0));
        let z = g(s * s);
        let p = g(s * horner(
            z,
            &[
                2.0,
                0.666_666_7,
                0.4,
                0.285_714_3,
                0.222_222_2,
                0.181_818_18,
            ],
        ));
        let k = e as f32;
        g(g(k * 0.693_145_75) + g(g(k * 1.428_606_8e-6) + p))
    }
    pub fn pow(x: f32, y: f32) -> f32 {
        if x <= 0.0 {
            return 0.0;
        }
        exp(g(y * log(x)))
    }
    pub fn sincos(x: f32) -> [f32; 2] {
        let k = g(g(x * 0.636_619_8).round_ties_even());
        let r = g(g(g(x - g(k * 1.570_312_5)) - g(k * 4.837_513e-4)) - g(k * 7.549_79e-8));
        let z = g(r * r);
        let s = g(r * horner(
            z,
            &[
                1.0,
                -0.166_666_67,
                0.008_333_333,
                -0.000_198_412_7,
                2.755_732e-6,
            ],
        ));
        let c = horner(
            z,
            &[
                1.0,
                -0.5,
                0.041_666_668,
                -0.001_388_888_9,
                2.480_158_7e-5,
                -2.755_732e-7,
            ],
        );
        match (k as i32) & 3 {
            0 => [s, c],
            1 => [c, g(-s)],
            2 => [g(-s), g(-c)],
            _ => [g(-c), s],
        }
    }
    pub fn tanh(x: f32) -> f32 {
        let a = x.abs();
        let mut t = 1.0;
        if a <= 9.0 {
            t = g(1.0 - div(2.0, g(exp(g(2.0 * a)) + 1.0)));
        }
        if x < 0.0 { g(-t) } else { t }
    }
    pub fn acosh(x: f32) -> f32 {
        if x <= 1.0 {
            return 0.0;
        }
        log(g(x + sqrt(g(g(x * x) - 1.0))))
    }
    pub fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
        let t = div(g(x - low), g(high - low)).clamp(0.0, 1.0);
        g(g(t * t) * g(3.0 - g(2.0 * t)))
    }
    /// What the probe writes for one input.
    pub fn probe([a, b, c, d]: [f32; 4]) -> [f32; 18] {
        [
            g(a + b),
            g(a - b),
            g(a * b),
            g(g(a * b) + c),
            g(g(a + b) + c),
            g(g(a * 1e-20) * 1e-20),
            g(-g(a * 0.0)),
            div(a, b),
            sqrt(a.abs()),
            exp(c.clamp(-80.0, 80.0)),
            log(g(a.abs() + 1.0)),
            pow(g(b.abs() + 0.5), c.clamp(-4.0, 4.0)),
            sincos(c)[0],
            sincos(c)[1],
            tanh(d),
            acosh(g(d.abs() + 1.0)),
            smoothstep(-1.0, 1.0, d),
            (d.to_bits() | 0x0100_0001) as f32,
        ]
    }
}

/// Inputs for the probe: ties between neighbouring floats, values that cancel, a fused
/// multiply-add's tell-tale (a * b + c where the product's rounding matters), and a spread of
/// magnitudes and signs from a fixed sequence.
fn probe_inputs() -> Vec<[f32; 4]> {
    let mut v = vec![
        [1.0, f32::EPSILON / 2.0, -1.0, 0.5],
        [
            1.0 + f32::EPSILON,
            1.0 + f32::EPSILON,
            -(1.0 + 2.0 * f32::EPSILON),
            -0.5,
        ],
        [3.0, 1.0 / 3.0, -1.0, 9.5],
        [0.1, 0.2, -0.3, -9.5],
        [16_777_216.0, 1.0, 0.0, 100.0],
        [-0.0, 2.0, 1.0e-30, 1.0e-30],
        [1.0e-30, 1.0e-30, 3.0, -3.0],
    ];
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..4089 {
        let mut x = [0.0f32; 4];
        for (i, slot) in x.iter_mut().enumerate() {
            let r = next();
            let unit = (r >> 40) as f32 / (1u64 << 24) as f32;
            let scale = [1.0, 100.0, 4.0, 3.0][i] * if r & 1 == 1 { -1.0 } else { 1.0 };
            let exponent = ((r >> 8) % 24) as i32 - 12;
            *slot = cpu::g(unit * scale * 2f32.powi(if i == 0 { exponent } else { 0 }));
        }
        v.push(x);
    }
    v
}

#[test]
fn the_kernels_arithmetic_is_the_cpus_bit_for_bit() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/water_det_probe.wgsl"),
    )
    .unwrap();
    let wgsl = determinize(&source, "p.count >> 30u").unwrap();
    let inputs = probe_inputs();
    let expected: Vec<[f32; 18]> = inputs.iter().map(|&x| cpu::probe(x)).collect();
    for backend in amoris_render::BackendChoice::under_test() {
        let gpu = amoris_render::Gpu::headless(backend).expect("the probe needs a GPU");
        use wgpu::util::DeviceExt;
        let device = &gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("deterministic probe"),
            source: wgpu::ShaderSource::Wgsl(wgsl.as_str().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("deterministic probe"),
            layout: None,
            module: &module,
            entry_point: Some("probe"),
            compilation_options: Default::default(),
            cache: None,
        });
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            // Params { count: u32, unused: vec3u }: 32 bytes.
            contents: bytemuck::cast_slice(&[inputs.len() as u32, 0, 0, 0, 0, 0, 0, 0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&inputs),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bytes = (inputs.len() * 18 * 4) as u64;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(inputs.len().div_ceil(64) as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
        gpu.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let got: Vec<[f32; 18]> = bytemuck::cast_slice(&slice.get_mapped_range().unwrap()).to_vec();
        let mut wrong = Vec::new();
        for (i, (got, want)) in got.iter().zip(&expected).enumerate() {
            for k in 0..18 {
                if got[k].to_bits() != want[k].to_bits() && wrong.len() < 8 {
                    wrong.push(format!(
                        "input {i} {:?}, output {k}: GPU {} ({:08x}), CPU {} ({:08x})",
                        inputs[i],
                        got[k],
                        got[k].to_bits(),
                        want[k],
                        want[k].to_bits()
                    ));
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "{} ({:?}): {wrong:#?}",
            gpu.info.name,
            gpu.info.backend
        );
        eprintln!(
            "{} ({:?}): {} inputs x 18 results, bit for bit",
            gpu.info.name,
            gpu.info.backend,
            inputs.len()
        );
    }
}

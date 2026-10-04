//! A stopped game thread, inspected (docs/spec/debugger.md 5): call frames with their positions
//! and scopes, values as CDP `RemoteObject`s and as JSON previews for agents, property listings,
//! evaluation on a frame and `Runtime.callFunctionOn`.
//!
//! An [`Inspector`] lives for one pause, on the game thread, inside the trace call that stopped
//! it. Every value it hands out (as an `objectId`) is held until the pause ends. Property listings
//! read descriptors and never run a getter or a Proxy trap; evaluations run the project's code, as
//! any debugger's do, under the script host's guard (`pocket_script::debug::guarded`).

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::ptr::NonNull;
use std::sync::Arc;

use pocket_script::debug::{FrameInfo, Guard, frame_info, guarded, stack_depth};
use pocket_script::ffi;
use rquickjs::function::{Rest, This};
use rquickjs::{Ctx, Function, Type, Value, qjs};
use serde_json::{Value as Json, json};

use crate::scripts::Script;

/// Properties a listing returns at most.
const MAX_PROPERTIES: usize = 2000;
/// Entries a preview shows.
const PREVIEW_ENTRIES: usize = 5;
/// Entries and depth of an agent's JSON preview.
const JSON_ENTRIES: usize = 48;
const JSON_DEPTH: usize = 3;

/// Named values: a scope's variables.
type Vars<'js> = Vec<(String, Value<'js>)>;

enum Handle<'js> {
    Value(Value<'js>),
    /// A scope: its variables, listed by `Runtime.getProperties`.
    Scope(Vec<(String, Value<'js>)>),
}

/// One stopped frame.
#[derive(Clone, Debug)]
pub struct Frame {
    /// QuickJS-ng's frame level (0: innermost); the CDP `callFrameId`.
    pub level: usize,
    pub function: String,
    pub script: Arc<Script>,
    /// 0-based position in the module's JavaScript.
    pub js: (u32, u32),
    /// 0-based TypeScript position, `None` in generated code.
    pub ts: Option<(u32, u32)>,
    /// Arguments and locals, then closure variables, as agents see them.
    pub locals: Vec<Variable>,
    pub closure: Vec<Variable>,
    /// A frame that has returned (a data breakpoint found at the end of a call): position only.
    pub returned: bool,
}

/// A variable as agents see it.
#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    pub kind: String,
    pub value: Json,
    /// An object's one-line description (`Float64Array(1)`, `Array(3)`, `Object`), as CDP
    /// describes it; `None` for primitives, whose `value` says it all.
    pub description: Option<String>,
}

impl Variable {
    pub fn json(&self) -> Json {
        let mut v = json!({"name": self.name, "type": self.kind, "value": self.value});
        if let Some(d) = &self.description {
            v["description"] = json!(d);
        }
        v
    }
}

/// The innermost frame's position, from the trace call (callers' come from their frames).
pub struct Top {
    pub script: Arc<Script>,
    pub line: u32,
    pub column: u32,
}

pub struct Inspector<'js> {
    ctx: *mut qjs::JSContext,
    rctx: Ctx<'js>,
    /// Handles of this pause, by `objectId`.
    handles: HashMap<String, Handle<'js>>,
    prefix: String,
    next: u64,
}

impl<'js> Inspector<'js> {
    /// An inspector for the pause numbered `serial`.
    ///
    /// # Safety
    /// `ctx` must be the stopped context, on its thread, inside the trace call.
    pub unsafe fn new(ctx: *mut qjs::JSContext, serial: u64) -> Inspector<'js> {
        // SAFETY: the caller's contract.
        let rctx = unsafe { Ctx::from_raw(NonNull::new(ctx).expect("a stopped context")) };
        Inspector {
            ctx,
            rctx,
            handles: HashMap::new(),
            prefix: format!("p{serial}:"),
            next: 0,
        }
    }

    fn id(&mut self, kind: &str) -> String {
        self.next += 1;
        format!("{}{kind}{}", self.prefix, self.next)
    }

    fn hold(&mut self, v: Value<'js>) -> String {
        let id = self.id("o");
        self.handles.insert(id.clone(), Handle::Value(v));
        id
    }

    fn hold_scope(&mut self, vars: Vec<(String, Value<'js>)>) -> String {
        let id = self.id("s");
        self.handles.insert(id.clone(), Handle::Scope(vars));
        id
    }

    fn own(&self, raw: qjs::JSValue) -> Value<'js> {
        // SAFETY: `raw` is a new reference in this context.
        unsafe { Value::from_raw(self.rctx.clone(), raw) }
    }

    fn borrowed(&self, raw: qjs::JSValue) -> Value<'js> {
        self.own(unsafe { qjs::JS_DupValue(self.ctx, raw) })
    }

    /// The stopped frames, innermost first: the project's bytecode frames (native frames and the
    /// prelude's are left out, as V8 leaves out natives), each with its scopes; and the same as
    /// CDP `callFrames`.
    pub fn capture(
        &mut self,
        top: &Top,
        script_of: &dyn Fn(&str) -> Option<Arc<Script>>,
    ) -> (Vec<Frame>, Json) {
        let depth = unsafe { stack_depth(self.ctx) };
        let mut frames = Vec::new();
        let mut cdp = Vec::new();
        for level in 0..depth {
            let Some(FrameInfo::Script {
                file,
                function,
                line,
                column,
            }) = (unsafe { frame_info(self.ctx, level) })
            else {
                continue;
            };
            let Some(script) = script_of(&file) else {
                continue;
            };
            let (line, column) = if level == 0 && Arc::ptr_eq(&script, &top.script) {
                (top.line, top.column)
            } else {
                (line, column)
            };
            let js = (line.saturating_sub(1), column.saturating_sub(1));
            let (local, closure) = self.variables(level);
            let preview = |me: &Self, vars: &[(String, Value<'js>)]| -> Vec<Variable> {
                vars.iter()
                    .map(|(n, v)| Variable {
                        name: n.clone(),
                        kind: kind_of(v).to_owned(),
                        value: me.json(v, 0),
                        description: (v.is_object() || v.is_function())
                            .then(|| me.describe(v)["description"].as_str().map(str::to_owned))
                            .flatten(),
                    })
                    .collect()
            };
            let locals = preview(self, &local);
            let closure_vars = preview(self, &closure);
            let local_id = self.hold_scope(local);
            let closure_id = self.hold_scope(closure);
            let global = self.own(unsafe { qjs::JS_GetGlobalObject(self.ctx) });
            let global_id = self.hold(global);
            let scope = |kind: &str, id: &str, name: &str| {
                json!({"type": kind, "name": name, "object": {"type": "object",
                       "className": "Object", "description": name, "objectId": id}})
            };
            cdp.push(json!({
                "callFrameId": level.to_string(),
                "functionName": function,
                "location": {"scriptId": script.id, "lineNumber": js.0, "columnNumber": js.1},
                "url": script.url,
                "scopeChain": [scope("local", &local_id, "Local"),
                               scope("closure", &closure_id, "Closure"),
                               scope("global", &global_id, "Global")],
                "this": {"type": "undefined"},
                "canBeRestarted": false,
            }));
            frames.push(Frame {
                level,
                function,
                ts: script.to_ts(js.0, js.1),
                script,
                js,
                locals,
                closure: closure_vars,
                returned: false,
            });
        }
        (frames, Json::Array(cdp))
    }

    /// Arguments and locals, then closure variables, of the frame at `level`.
    fn variables(&self, level: usize) -> (Vars<'js>, Vars<'js>) {
        let (mut local, mut closure) = (Vec::new(), Vec::new());
        let mut vars: *mut ffi::JSDebugLocalVar = std::ptr::null_mut();
        let mut count = 0;
        let level = i32::try_from(level).unwrap_or(i32::MAX);
        unsafe {
            if ffi::JS_GetLocalVariablesAtLevel(self.ctx, level, &mut vars, &mut count) < 0 {
                let _ = self.own(qjs::JS_GetException(self.ctx));
                return (local, closure);
            }
            for i in 0..usize::try_from(count).unwrap_or(0) {
                let var = &*vars.add(i);
                let name = CStr::from_ptr(var.name).to_string_lossy().into_owned();
                let value = self.borrowed(var.value);
                if var.is_closure {
                    closure.push((name, value));
                } else {
                    local.push((name, value));
                }
            }
            ffi::JS_FreeLocalVariables(self.ctx, vars, count);
        }
        (local, closure)
    }

    /// A CDP `RemoteObject` for `v`; an object gets an `objectId` valid for this pause.
    pub fn remote(&mut self, v: &Value<'js>) -> Json {
        let mut r = self.describe(v);
        if v.is_object() || v.is_symbol() {
            r["objectId"] = json!(self.hold(v.clone()));
            if v.is_object() && !v.is_function() && !v.is_proxy() {
                r["preview"] = self.preview(v);
            }
        }
        r
    }

    /// A `RemoteObject` without an id.
    fn describe(&self, v: &Value<'js>) -> Json {
        match v.type_of() {
            Type::Uninitialized | Type::Undefined => json!({"type": "undefined"}),
            Type::Null => json!({"type": "object", "subtype": "null", "value": null}),
            Type::Bool => {
                let b = v.as_bool().unwrap_or(false);
                json!({"type": "boolean", "value": b, "description": b.to_string()})
            }
            Type::Int => {
                let i = v.as_int().unwrap_or(0);
                json!({"type": "number", "value": i, "description": i.to_string()})
            }
            Type::Float => number(v.as_float().unwrap_or(f64::NAN)),
            Type::String => {
                let s: String = v.get().unwrap_or_default();
                json!({"type": "string", "value": s})
            }
            Type::BigInt => {
                let s = self.text(v);
                json!({"type": "bigint", "unserializableValue": format!("{s}n"),
                       "description": format!("{s}n")})
            }
            Type::Symbol => json!({"type": "symbol", "description": self.text(v)}),
            Type::Function | Type::Constructor => {
                let name = self.data_string(v, "name").unwrap_or_default();
                json!({"type": "function", "className": "Function",
                       "description": format!("function {name}() {{ [code] }}")})
            }
            Type::Array => {
                let n = self.array_len(v);
                json!({"type": "object", "subtype": "array", "className": "Array",
                       "description": format!("Array({n})")})
            }
            Type::Exception => {
                let name = self.class_name(v);
                let desc = self
                    .data_string(v, "stack")
                    .filter(|s| !s.is_empty())
                    .map(|stack| format!("{}\n{stack}", self.text(v)))
                    .unwrap_or_else(|| self.text(v));
                json!({"type": "object", "subtype": "error", "className": name,
                       "description": desc})
            }
            Type::Proxy => json!({"type": "object", "subtype": "proxy", "className": "Object",
                                  "description": "Proxy"}),
            Type::Promise => json!({"type": "object", "subtype": "promise",
                                    "className": "Promise", "description": "Promise"}),
            _ => {
                if let Some(kind) = typed_array_name(v) {
                    let n = self.array_len(v);
                    json!({"type": "object", "subtype": "typedarray", "className": kind,
                           "description": format!("{kind}({n})")})
                } else {
                    let class = self.class_name(v);
                    json!({"type": "object", "className": class, "description": class})
                }
            }
        }
    }

    /// A CDP `ObjectPreview`: the first own data properties, described in a line each.
    fn preview(&self, v: &Value<'js>) -> Json {
        let d = self.describe(v);
        let props = self.own_properties(v, true);
        let overflow = props.len() > PREVIEW_ENTRIES;
        let entries: Vec<Json> = props
            .into_iter()
            .take(PREVIEW_ENTRIES)
            .map(|(name, p, _)| match p {
                Prop::Data(x) => {
                    let xd = self.describe(&x);
                    let value = match &xd["value"] {
                        Json::String(s) => s.clone(),
                        Json::Null if xd["type"] == "undefined" => "undefined".into(),
                        Json::Null => xd["description"].as_str().unwrap_or("").to_owned(),
                        other => other.to_string(),
                    };
                    let mut e = json!({"name": name, "type": xd["type"], "value": value});
                    if let Some(st) = xd.get("subtype") {
                        e["subtype"] = st.clone();
                    }
                    e
                }
                Prop::Accessor { .. } => json!({"name": name, "type": "accessor"}),
            })
            .collect();
        let mut p = json!({"type": d["type"], "description": d["description"],
                           "overflow": overflow, "properties": entries});
        if let Some(st) = d.get("subtype") {
            p["subtype"] = st.clone();
        }
        p
    }

    /// A value as agents see it: JSON, objects to a few levels and entries.
    pub fn json(&self, v: &Value<'js>, depth: usize) -> Json {
        match v.type_of() {
            Type::Uninitialized | Type::Undefined => {
                if depth == 0 {
                    Json::Null
                } else {
                    json!("undefined")
                }
            }
            Type::Null => Json::Null,
            Type::Bool => json!(v.as_bool().unwrap_or(false)),
            Type::Int => json!(v.as_int().unwrap_or(0)),
            Type::Float => number_json(v.as_float().unwrap_or(f64::NAN)),
            Type::String => json!(v.get::<String>().unwrap_or_default()),
            Type::BigInt => json!(format!("{}n", self.text(v))),
            Type::Symbol => json!(self.text(v)),
            Type::Function | Type::Constructor => {
                json!(format!(
                    "[function {}]",
                    self.data_string(v, "name").unwrap_or_default()
                ))
            }
            Type::Exception => json!(self.text(v)),
            Type::Proxy => json!("[proxy]"),
            Type::Promise => json!("[promise]"),
            _ if depth >= JSON_DEPTH => {
                json!(
                    self.describe(v)["description"]
                        .as_str()
                        .map(|d| format!("[{d}]"))
                )
            }
            Type::Array => {
                let items: Vec<Json> = self
                    .own_properties(v, true)
                    .into_iter()
                    .filter(|(n, _, _)| n.parse::<u32>().is_ok())
                    .take(JSON_ENTRIES)
                    .map(|(_, p, _)| self.prop_json(p, depth))
                    .collect();
                Json::Array(items)
            }
            _ => {
                if typed_array_name(v).is_some() {
                    let items: Vec<Json> = self
                        .own_properties(v, true)
                        .into_iter()
                        .filter(|(n, _, _)| n.parse::<u32>().is_ok())
                        .take(JSON_ENTRIES)
                        .map(|(_, p, _)| self.prop_json(p, depth))
                        .collect();
                    return Json::Array(items);
                }
                let mut out = serde_json::Map::new();
                for (name, p, enumerable) in self.own_properties(v, true) {
                    if !enumerable {
                        continue;
                    }
                    if out.len() >= JSON_ENTRIES {
                        out.insert("…".into(), json!("more"));
                        break;
                    }
                    out.insert(name, self.prop_json(p, depth));
                }
                Json::Object(out)
            }
        }
    }

    fn prop_json(&self, p: Prop<'js>, depth: usize) -> Json {
        match p {
            Prop::Data(x) => self.json(&x, depth + 1),
            Prop::Accessor { .. } => json!("[getter]"),
        }
    }

    /// `Runtime.getProperties` of an object or a scope of this pause.
    pub fn properties(&mut self, object_id: &str, own_only: bool) -> Result<Json, String> {
        let _ = own_only;
        let listed: Vec<(String, Prop<'js>, bool)> = match self.handles.get(object_id) {
            None => return Err(format!("No object {object_id} in this pause.")),
            Some(Handle::Scope(vars)) => vars
                .iter()
                .map(|(n, v)| (n.clone(), Prop::Data(v.clone()), true))
                .collect(),
            Some(Handle::Value(v)) => {
                let v = v.clone();
                if v.is_proxy() || !v.is_object() {
                    Vec::new()
                } else {
                    self.own_properties(&v, false)
                }
            }
        };
        let mut result = Vec::new();
        for (name, p, enumerable) in listed {
            let entry = match p {
                Prop::Data(v) => {
                    let value = self.remote(&v);
                    json!({"name": name, "value": value, "writable": true, "configurable": true,
                           "enumerable": enumerable, "isOwn": true})
                }
                Prop::Accessor { get, set } => {
                    let get = self.remote(&get);
                    let set = self.remote(&set);
                    json!({"name": name, "get": get, "set": set, "configurable": true,
                           "enumerable": enumerable, "isOwn": true})
                }
            };
            result.push(entry);
        }
        Ok(json!({"result": result, "internalProperties": []}))
    }

    /// Own properties (strings only) read from their descriptors: no getter runs.
    fn own_properties(&self, v: &Value<'js>, data_first: bool) -> Vec<(String, Prop<'js>, bool)> {
        let _ = data_first;
        let mut out = Vec::new();
        if !v.is_object() || v.is_proxy() {
            return out;
        }
        let mut tab: *mut qjs::JSPropertyEnum = std::ptr::null_mut();
        let mut len: u32 = 0;
        let flags = qjs::JS_GPN_STRING_MASK as i32;
        let r =
            unsafe { qjs::JS_GetOwnPropertyNames(self.ctx, &mut tab, &mut len, v.as_raw(), flags) };
        if r < 0 {
            let _ = self.own(unsafe { qjs::JS_GetException(self.ctx) });
            return out;
        }
        for i in 0..len as usize {
            if out.len() >= MAX_PROPERTIES {
                break;
            }
            let atom = unsafe { (*tab.add(i)).atom };
            let name = unsafe { pocket_script::debug::atom_text(self.ctx, atom) };
            let mut desc = qjs::JSPropertyDescriptor {
                flags: 0,
                value: qjs::JS_UNDEFINED,
                getter: qjs::JS_UNDEFINED,
                setter: qjs::JS_UNDEFINED,
            };
            let found = unsafe { qjs::JS_GetOwnProperty(self.ctx, &mut desc, v.as_raw(), atom) };
            if found <= 0 {
                if found < 0 {
                    let _ = self.own(unsafe { qjs::JS_GetException(self.ctx) });
                }
                continue;
            }
            let value = self.own(desc.value);
            let get = self.own(desc.getter);
            let set = self.own(desc.setter);
            let enumerable = desc.flags & qjs::JS_PROP_ENUMERABLE as i32 != 0;
            let prop = if desc.flags & qjs::JS_PROP_GETSET as i32 != 0 {
                Prop::Accessor { get, set }
            } else {
                Prop::Data(value)
            };
            out.push((name, prop, enumerable));
        }
        unsafe { qjs::JS_FreePropertyEnum(self.ctx, tab, len) };
        out
    }

    /// Evaluates `expression` in the frame at `level`: the value, or the exception it threw.
    pub fn eval_value(&self, level: usize, expression: &str) -> Result<Value<'js>, Value<'js>> {
        eval_in_frame(
            &self.rctx,
            level,
            expression,
            Guard::Evaluate("a debugger evaluated an expression"),
        )
    }

    /// `Debugger.evaluateOnCallFrame`'s answer.
    pub fn evaluate(&mut self, level: usize, expression: &str, by_value: bool) -> Json {
        match self.eval_value(level, expression) {
            Ok(v) => {
                if by_value {
                    json!({"result": self.by_value(&v)})
                } else {
                    json!({"result": self.remote(&v)})
                }
            }
            Err(e) => self.exception_details(&e),
        }
    }

    /// An evaluation's answer as agents see it.
    pub fn evaluate_json(&self, level: usize, expression: &str) -> Json {
        match self.eval_value(level, expression) {
            Ok(v) => json!({"type": kind_of(&v), "value": self.json(&v, 0),
                            "description": self.describe(&v)["description"]}),
            Err(e) => json!({"error": self.text(&e)}),
        }
    }

    fn exception_details(&mut self, e: &Value<'js>) -> Json {
        let text = self.text(e);
        let remote = self.remote(e);
        json!({"result": remote, "exceptionDetails": {"exceptionId": 1, "text": text,
               "lineNumber": 0, "columnNumber": 0, "exception": remote}})
    }

    fn by_value(&self, v: &Value<'js>) -> Json {
        let mut d = self.describe(v);
        if v.is_object() {
            d["value"] = self.json(v, 0);
        }
        d
    }

    /// `Runtime.callFunctionOn`: `declaration` called with `this` = the object (or `undefined` for
    /// a scope) and the arguments (values, `unserializableValue`s or `objectId`s of this pause).
    pub fn call_function_on(
        &mut self,
        object_id: Option<&str>,
        declaration: &str,
        args: &[Json],
        by_value: bool,
    ) -> Result<Json, String> {
        let this = match object_id.map(|id| self.handles.get(id)) {
            Some(Some(Handle::Value(v))) => v.clone(),
            Some(Some(Handle::Scope(_))) | None => Value::new_undefined(self.rctx.clone()),
            Some(None) => {
                return Err(format!(
                    "No object {} in this pause.",
                    object_id.unwrap_or("")
                ));
            }
        };
        let mut values = Vec::new();
        for a in args {
            let v = if let Some(id) = a["objectId"].as_str() {
                match self.handles.get(id) {
                    Some(Handle::Value(v)) => v.clone(),
                    _ => return Err(format!("No object {id} in this pause.")),
                }
            } else if let Some(u) = a["unserializableValue"].as_str() {
                match eval_global(&self.rctx, u) {
                    Ok(v) => v,
                    Err(e) => return Ok(self.exception_details(&e)),
                }
            } else if a.get("value").is_some() {
                self.rctx
                    .json_parse(a["value"].to_string())
                    .map_err(|e| e.to_string())?
            } else {
                Value::new_undefined(self.rctx.clone())
            };
            values.push(v);
        }
        let ctx = self.rctx.clone();
        let called = unsafe {
            guarded(self.ctx, Guard::Read, || {
                let f: Function = match eval_global(&ctx, &format!("({declaration})")) {
                    Ok(v) => match v.into_function() {
                        Some(f) => f,
                        None => return Err(Value::new_undefined(ctx.clone())),
                    },
                    Err(e) => return Err(e),
                };
                f.call::<_, Value>((This(this), Rest(values)))
                    .map_err(|_| ctx.catch())
            })
        };
        match called {
            Ok(v) => {
                if by_value {
                    Ok(json!({"result": self.by_value(&v)}))
                } else {
                    Ok(json!({"result": self.remote(&v)}))
                }
            }
            Err(e) => Ok(self.exception_details(&e)),
        }
    }

    /// `Debugger.setVariableValue`: sets a variable of the frame at `level` to a CDP
    /// `CallArgument`.
    pub fn set_variable(&mut self, level: usize, name: &str, arg: &Json) -> Result<Json, String> {
        let value = if let Some(id) = arg["objectId"].as_str() {
            match self.handles.get(id) {
                Some(Handle::Value(v)) => v.clone(),
                _ => return Err(format!("No object {id} in this pause.")),
            }
        } else if let Some(u) = arg["unserializableValue"].as_str() {
            eval_global(&self.rctx, u).map_err(|e| self.text(&e))?
        } else if arg.get("value").is_some() {
            self.rctx
                .json_parse(arg["value"].to_string())
                .map_err(|e| e.to_string())?
        } else {
            Value::new_undefined(self.rctx.clone())
        };
        self.set_at_level(level, name, &value)?;
        Ok(json!({}))
    }

    /// The agents' `debug.set`: `expression`, evaluated in the frame at `level`, becomes the value
    /// of the frame's variable `name`; answers the value as `debug.eval` does. An assignment
    /// evaluated in the frame (`debug.eval "r = 7"`) does not do this: QuickJS hands an evaluation
    /// a copy of each local no closure captured, so only writes through an object reach the frame.
    pub fn assign(&self, level: usize, name: &str, expression: &str) -> Result<Json, String> {
        let value = self
            .eval_value(level, expression)
            .map_err(|e| format!("The expression threw: {}", self.text(&e)))?;
        let shown = json!({"type": kind_of(&value), "value": self.json(&value, 0),
                           "description": self.describe(&value)["description"]});
        self.set_at_level(level, name, &value)?;
        Ok(shown)
    }

    /// Sets the variable `name` of the frame at `level` (its stack slot, or the closure's cell),
    /// tainting the run when it did. A name declared in two block scopes of one function names the
    /// first declaration: QuickJS keeps no scope ranges at run time.
    fn set_at_level(&self, level: usize, name: &str, value: &Value<'js>) -> Result<(), String> {
        let cname = CString::new(name).map_err(|e| e.to_string())?;
        let level = i32::try_from(level).map_err(|e| e.to_string())?;
        let raw = unsafe { qjs::JS_DupValue(self.ctx, value.as_raw()) };
        match unsafe { ffi::JS_SetVariableAtLevel(self.ctx, level, cname.as_ptr(), raw) } {
            0 => {
                unsafe { pocket_script::debug::taint(self.ctx, "a debugger set a variable") };
                Ok(())
            }
            -2 => Err(format!("{name} is a constant.")),
            _ => Err(format!("No variable {name} in this frame.")),
        }
    }

    /// A value's text (`String(v)` for primitives and errors; no project code runs for others).
    pub fn text(&self, v: &Value<'js>) -> String {
        if v.is_object() && !v.is_error() {
            return self.describe(v)["description"]
                .as_str()
                .unwrap_or("")
                .to_owned();
        }
        unsafe {
            let p = qjs::JS_ToCStringLen2(self.ctx, std::ptr::null_mut(), v.as_raw(), false);
            if p.is_null() {
                let _ = self.own(qjs::JS_GetException(self.ctx));
                return String::new();
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            qjs::JS_FreeCString(self.ctx, p);
            s
        }
    }

    fn data_string(&self, v: &Value<'js>, key: &str) -> Option<String> {
        self.own_properties(v, true)
            .into_iter()
            .find(|(n, _, _)| n == key)
            .and_then(|(_, p, _)| match p {
                Prop::Data(x) => x.get::<String>().ok(),
                Prop::Accessor { .. } => None,
            })
    }

    fn array_len(&self, v: &Value<'js>) -> u64 {
        let mut n: i64 = 0;
        unsafe { qjs::JS_GetLength(self.ctx, v.as_raw(), &mut n) };
        u64::try_from(n).unwrap_or(0)
    }

    /// The constructor's name along the prototype chain, read from descriptors.
    fn class_name(&self, v: &Value<'js>) -> String {
        let mut proto = self.own(unsafe { qjs::JS_GetPrototype(self.ctx, v.as_raw()) });
        for _ in 0..8 {
            if !proto.is_object() || proto.is_proxy() {
                break;
            }
            let ctor = self
                .own_properties(&proto, true)
                .into_iter()
                .find(|(n, _, _)| n == "constructor")
                .and_then(|(_, p, _)| match p {
                    Prop::Data(c) => Some(c),
                    Prop::Accessor { .. } => None,
                });
            if let Some(c) = ctor
                && let Some(name) = self.data_string(&c, "name").filter(|n| !n.is_empty())
            {
                return name;
            }
            proto = self.own(unsafe { qjs::JS_GetPrototype(self.ctx, proto.as_raw()) });
        }
        "Object".into()
    }
}

enum Prop<'js> {
    Data(Value<'js>),
    Accessor { get: Value<'js>, set: Value<'js> },
}

/// Evaluates in the frame at `level`, under the host's guard (the trace handler is cleared by
/// the caller during a pause): the natives that write refuse, and `guard` says whether it taints
/// the run.
pub fn eval_in_frame<'js>(
    ctx: &Ctx<'js>,
    level: usize,
    expression: &str,
    guard: Guard,
) -> Result<Value<'js>, Value<'js>> {
    let raw = ctx.as_raw().as_ptr();
    let Ok(input) = CString::new(expression) else {
        return Err(Value::new_undefined(ctx.clone()));
    };
    let level = i32::try_from(level).unwrap_or(i32::MAX);
    let v = unsafe {
        guarded(raw, guard, || {
            ffi::JS_EvalInStackFrame(
                raw,
                level,
                input.as_ptr(),
                expression.len(),
                c"<debugger>".as_ptr(),
            )
        })
    };
    if unsafe { qjs::JS_IsException(v) } {
        Err(ctx.catch())
    } else {
        Ok(unsafe { Value::from_raw(ctx.clone(), v) })
    }
}

/// Evaluates a script in the global scope, under the host's guard.
fn eval_global<'js>(ctx: &Ctx<'js>, source: &str) -> Result<Value<'js>, Value<'js>> {
    let raw = ctx.as_raw().as_ptr();
    let Ok(input) = CString::new(source) else {
        return Err(Value::new_undefined(ctx.clone()));
    };
    let v = unsafe {
        guarded(raw, Guard::Read, || {
            qjs::JS_Eval(
                raw,
                input.as_ptr(),
                source.len() as _,
                c"<debugger>".as_ptr(),
                qjs::JS_EVAL_TYPE_GLOBAL as i32,
            )
        })
    };
    if unsafe { qjs::JS_IsException(v) } {
        Err(ctx.catch())
    } else {
        Ok(unsafe { Value::from_raw(ctx.clone(), v) })
    }
}

/// Whether a breakpoint's condition holds in the innermost frame (an exception counts as false).
///
/// # Safety
/// `ctx` must be the stopped context, on its thread, with tracing suspended.
pub unsafe fn condition_holds(ctx: *mut qjs::JSContext, condition: &str) -> bool {
    let Some(raw) = NonNull::new(ctx) else {
        return false;
    };
    let rctx = unsafe { Ctx::from_raw(raw) };
    match eval_in_frame(
        &rctx,
        0,
        condition,
        Guard::Evaluate("a breakpoint condition ran"),
    ) {
        Ok(v) => unsafe { qjs::JS_ToBool(ctx, v.as_raw()) > 0 },
        Err(_) => false,
    }
}

/// A logpoint's message: `template` as a template literal in the innermost frame.
///
/// # Safety
/// As [`condition_holds`].
pub unsafe fn log_message(ctx: *mut qjs::JSContext, template: &str) -> String {
    let Some(raw) = NonNull::new(ctx) else {
        return String::new();
    };
    let rctx = unsafe { Ctx::from_raw(raw) };
    let escaped = template.replace('\\', "\\\\").replace('`', "\\`");
    let insp = unsafe { Inspector::new(ctx, 0) };
    match eval_in_frame(
        &rctx,
        0,
        &format!("`{escaped}`"),
        Guard::Evaluate("a logpoint's message ran"),
    ) {
        Ok(v) => insp.text(&v),
        Err(e) => format!("(logpoint failed: {})", insp.text(&e)),
    }
}

/// The JavaScript type agents see (`typeof`, with `null` and `array`).
pub fn kind_of(v: &Value<'_>) -> &'static str {
    match v.type_of() {
        Type::Uninitialized | Type::Undefined => "undefined",
        Type::Null => "null",
        Type::Bool => "boolean",
        Type::Int | Type::Float => "number",
        Type::String => "string",
        Type::BigInt => "bigint",
        Type::Symbol => "symbol",
        Type::Function | Type::Constructor => "function",
        Type::Array => "array",
        _ => "object",
    }
}

fn typed_array_name(v: &Value<'_>) -> Option<&'static str> {
    let t = unsafe { qjs::JS_GetTypedArrayType(v.as_raw()) };
    Some(match t {
        0 => "Uint8ClampedArray",
        1 => "Int8Array",
        2 => "Uint8Array",
        3 => "Int16Array",
        4 => "Uint16Array",
        5 => "Int32Array",
        6 => "Uint32Array",
        7 => "BigInt64Array",
        8 => "BigUint64Array",
        9 => "Float16Array",
        10 => "Float32Array",
        11 => "Float64Array",
        _ => return None,
    })
}

/// A number as agents see it: an integer when it is one (JavaScript does not tell 15 from 15.0),
/// and NaN, the infinities and -0 as their text.
pub fn number_json(f: f64) -> Json {
    if f.is_finite() && !(f == 0.0 && f.is_sign_negative()) {
        if f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
            json!(f as i64)
        } else {
            json!(f)
        }
    } else {
        number(f)["description"].clone()
    }
}

/// A CDP number `RemoteObject`.
pub fn number(f: f64) -> Json {
    if f.is_nan() {
        json!({"type": "number", "unserializableValue": "NaN", "description": "NaN"})
    } else if f.is_infinite() {
        let s = if f > 0.0 { "Infinity" } else { "-Infinity" };
        json!({"type": "number", "unserializableValue": s, "description": s})
    } else if f == 0.0 && f.is_sign_negative() {
        json!({"type": "number", "unserializableValue": "-0", "description": "-0"})
    } else {
        json!({"type": "number", "value": f, "description": format_number(f)})
    }
}

/// A number as JavaScript prints it, near enough (integers without a fraction).
fn format_number(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e21 {
        format!("{f:.0}")
    } else {
        f.to_string()
    }
}

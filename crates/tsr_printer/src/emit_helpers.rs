//! Emit helpers (`tsc/internal/printer/helpers.go`): the runtime functions a
//! transform asks the printer to write ahead of the code that calls them.
//!
//! The helper texts below are generated from the pinned Go source's raw string
//! literals, not retyped; `emit_helper_texts_match_the_pinned_go` checks every
//! text's length and hash against the compiled Go package's values.

use std::fmt;

/// Go's `Priority`: helpers with a higher priority are emitted earlier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Priority {
    pub value: i64,
}

/// Go's `TextCallback`: the scoped helpers build their text around a name the
/// printer makes unique in the current scope.
pub type EmitHelperTextCallback = fn(make_unique_name: &mut dyn FnMut(&[u8]) -> Vec<u8>) -> Vec<u8>;

/// Go's `EmitHelper`. Upstream compares helpers by pointer, so identity here is
/// the helper's address: every helper is a `static`, and equality is
/// `std::ptr::eq`.
pub struct EmitHelper {
    /// A unique name for this helper.
    pub name: &'static [u8],
    /// Indicates whether the helper MUST be emitted in the current scope.
    pub scoped: bool,
    /// ES3-compatible raw script text.
    pub text: &'static [u8],
    /// A function yielding an ES3-compatible raw script text.
    pub text_callback: Option<EmitHelperTextCallback>,
    /// Helpers with a higher priority are emitted earlier than other helpers on the node.
    pub priority: Option<Priority>,
    /// Emit helpers this helper depends on.
    pub dependencies: &'static [&'static EmitHelper],
    /// The name of the helper to use when importing via `--importHelpers`.
    pub import_name: &'static [u8],
}

impl PartialEq for EmitHelper {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
impl Eq for EmitHelper {}

impl fmt::Debug for EmitHelper {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output
            .debug_struct("EmitHelper")
            .field("name", &String::from_utf8_lossy(self.name))
            .field("scoped", &self.scoped)
            .field("priority", &self.priority)
            .finish_non_exhaustive()
    }
}

/// Go returns the priority difference as an `int` for `slices.SortStableFunc`;
/// a caller sorting with Rust's `sort_by` compares the result with zero.
/// Upstream compares the `*Priority` pointers first: equal only when both are
/// nil (or the same helper), which this reproduces with `None == None`.
// port: tsc/internal/printer/helpers.go:compareEmitHelpers
pub fn compare_emit_helpers(x: &EmitHelper, y: &EmitHelper) -> i64 {
    if x == y {
        return 0;
    }
    match (x.priority, y.priority) {
        (None, None) => 0,
        (None, Some(_)) => 1,
        (Some(_), None) => -1,
        (Some(x), Some(y)) => x.value - y.value,
    }
}

// The scoped helpers' `TextCallback` closures.
fn async_super_helper_text(make_unique_name: &mut dyn FnMut(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let mut text = b"\nconst ".to_vec();
    text.extend_from_slice(&make_unique_name(b"_superIndex"));
    text.extend_from_slice(b" = name => super[name];");
    text
}

fn advanced_async_super_helper_text(make_unique_name: &mut dyn FnMut(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let mut text = b"\nconst ".to_vec();
    text.extend_from_slice(&make_unique_name(b"_superIndex"));
    text.extend_from_slice(b" = (function (geti, seti) {\n");
    text.extend_from_slice(b"    const cache = Object.create(null);\n");
    text.extend_from_slice(b"    return name => cache[name] || (cache[name] = { get value() { return geti(name); }, set value(v) { seti(name, v); } });\n");
    text.extend_from_slice(b"})(name => super[name], (name, value) => super[name] = value);");
    text
}

// Generated from helpers.go by extracting each raw string literal.
pub(crate) static DECORATE_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:decorate",
    scoped: false,
    text: br#"var __decorate = (this && this.__decorate) || function (decorators, target, key, desc) {
    var c = arguments.length, r = c < 3 ? target : desc === null ? desc = Object.getOwnPropertyDescriptor(target, key) : desc, d;
    if (typeof Reflect === "object" && typeof Reflect.decorate === "function") r = Reflect.decorate(decorators, target, key, desc);
    else for (var i = decorators.length - 1; i >= 0; i--) if (d = decorators[i]) r = (c < 3 ? d(r) : c > 3 ? d(target, key, r) : d(target, key)) || r;
    return c > 3 && r && Object.defineProperty(target, key, r), r;
};"#,
    text_callback: None,
    priority: Some(Priority { value: 2 }),
    dependencies: &[],
    import_name: b"__decorate",
};

pub(crate) static METADATA_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:metadata",
    scoped: false,
    text: br#"var __metadata = (this && this.__metadata) || function (k, v) {
    if (typeof Reflect === "object" && typeof Reflect.metadata === "function") return Reflect.metadata(k, v);
};"#,
    text_callback: None,
    priority: Some(Priority { value: 3 }),
    dependencies: &[],
    import_name: b"__metadata",
};

pub(crate) static PARAM_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:param",
    scoped: false,
    text: br"var __param = (this && this.__param) || function (paramIndex, decorator) {
    return function (target, key) { decorator(target, key, paramIndex); }
};",
    text_callback: None,
    priority: Some(Priority { value: 4 }),
    dependencies: &[],
    import_name: b"__param",
};

pub(crate) static ADD_DISPOSABLE_RESOURCE_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:addDisposableResource",
    scoped: false,
    text: br#"var __addDisposableResource = (this && this.__addDisposableResource) || function (env, value, async) {
    if (value !== null && value !== void 0) {
        if (typeof value !== "object" && typeof value !== "function") throw new TypeError("Object expected.");
        var dispose, inner;
        if (async) {
            if (!Symbol.asyncDispose) throw new TypeError("Symbol.asyncDispose is not defined.");
            dispose = value[Symbol.asyncDispose];
        }
        if (dispose === void 0) {
            if (!Symbol.dispose) throw new TypeError("Symbol.dispose is not defined.");
            dispose = value[Symbol.dispose];
            if (async) inner = dispose;
        }
        if (typeof dispose !== "function") throw new TypeError("Object not disposable.");
        if (inner) dispose = function() { try { inner.call(this); } catch (e) { return Promise.reject(e); } };
        env.stack.push({ value: value, dispose: dispose, async: async });
    }
    else if (async) {
        env.stack.push({ async: true });
    }
    return value;
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__addDisposableResource",
};

pub(crate) static DISPOSE_RESOURCES_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:disposeResources",
    scoped: false,
    text: br#"var __disposeResources = (this && this.__disposeResources) || (function (SuppressedError) {
    return function (env) {
        function fail(e) {
            env.error = env.hasError ? new SuppressedError(e, env.error, "An error was suppressed during disposal.") : e;
            env.hasError = true;
        }
        var r, s = 0;
        function next() {
            while (r = env.stack.pop()) {
                try {
                    if (!r.async && s === 1) return s = 0, env.stack.push(r), Promise.resolve().then(next);
                    if (r.dispose) {
                        var result = r.dispose.call(r.value);
                        if (r.async) return s |= 2, Promise.resolve(result).then(next, function(e) { fail(e); return next(); });
                    }
                    else s |= 1;
                }
                catch (e) {
                    fail(e);
                }
            }
            if (s === 1) return env.hasError ? Promise.reject(env.error) : Promise.resolve();
            if (env.hasError) throw env.error;
        }
        return next();
    };
})(typeof SuppressedError === "function" ? SuppressedError : function (error, suppressed, message) {
    var e = new Error(message);
    return e.name = "SuppressedError", e.error = error, e.suppressed = suppressed, e;
});"#,
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__disposeResources",
};

pub(crate) static CLASS_PRIVATE_FIELD_GET_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:classPrivateFieldGet",
    scoped: false,
    text: br#"var __classPrivateFieldGet = (this && this.__classPrivateFieldGet) || function (receiver, state, kind, f) {
    if (kind === "a" && !f) throw new TypeError("Private accessor was defined without a getter");
    if (typeof state === "function" ? receiver !== state || !f : !state.has(receiver)) throw new TypeError("Cannot read private member from an object whose class did not declare it");
    return kind === "m" ? f : kind === "a" ? f.call(receiver) : f ? f.value : state.get(receiver);
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__classPrivateFieldGet",
};

pub(crate) static CLASS_PRIVATE_FIELD_SET_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:classPrivateFieldSet",
    scoped: false,
    text: br#"var __classPrivateFieldSet = (this && this.__classPrivateFieldSet) || function (receiver, state, value, kind, f) {
    if (kind === "m") throw new TypeError("Private method is not writable");
    if (kind === "a" && !f) throw new TypeError("Private accessor was defined without a setter");
    if (typeof state === "function" ? receiver !== state || !f : !state.has(receiver)) throw new TypeError("Cannot write private member to an object whose class did not declare it");
    return (kind === "a" ? f.call(receiver, value) : f ? f.value = value : state.set(receiver, value)), value;
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__classPrivateFieldSet",
};

pub(crate) static CLASS_PRIVATE_FIELD_IN_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:classPrivateFieldIn",
    scoped: false,
    text: br#"var __classPrivateFieldIn = (this && this.__classPrivateFieldIn) || function(state, receiver) {
    if (receiver === null || (typeof receiver !== "object" && typeof receiver !== "function")) throw new TypeError("Cannot use 'in' operator on non-object");
    return typeof state === "function" ? receiver === state : state.has(receiver);
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__classPrivateFieldIn",
};

pub(crate) static AWAIT_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:await",
    scoped: false,
    text: br"var __await = (this && this.__await) || function (v) { return this instanceof __await ? (this.v = v, this) : new __await(v); }",
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__await",
};

pub(crate) static ASYNC_GENERATOR_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:asyncGenerator",
    scoped: false,
    text: br#"var __asyncGenerator = (this && this.__asyncGenerator) || function (thisArg, _arguments, generator) {
    if (!Symbol.asyncIterator) throw new TypeError("Symbol.asyncIterator is not defined.");
    var g = generator.apply(thisArg, _arguments || []), i, q = [];
    return i = Object.create((typeof AsyncIterator === "function" ? AsyncIterator : Object).prototype), verb("next"), verb("throw"), verb("return", awaitReturn), i[Symbol.asyncIterator] = function () { return this; }, i;
    function awaitReturn(f) { return function (v) { return Promise.resolve(v).then(f, reject); }; }
    function verb(n, f) { if (g[n]) { i[n] = function (v) { return new Promise(function (a, b) { q.push([n, v, a, b]) > 1 || resume(n, v); }); }; if (f) i[n] = f(i[n]); } }
    function resume(n, v) { try { step(g[n](v)); } catch (e) { settle(q[0][3], e); } }
    function step(r) { r.value instanceof __await ? Promise.resolve(r.value.v).then(fulfill, reject) : settle(q[0][2], r); }
    function fulfill(value) { resume("next", value); }
    function reject(value) { resume("throw", value); }
    function settle(f, v) { if (f(v), q.shift(), q.length) resume(q[0][0], q[0][1]); }
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[&AWAIT_HELPER],
    import_name: b"__asyncGenerator",
};

pub(crate) static ASYNC_DELEGATOR_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:asyncDelegator",
    scoped: false,
    text: br#"var __asyncDelegator = (this && this.__asyncDelegator) || function (o) {
    var i, p;
    return i = {}, verb("next"), verb("throw", function (e) { throw e; }), verb("return"), i[Symbol.iterator] = function () { return this; }, i;
    function verb(n, f) { i[n] = o[n] ? function (v) { return (p = !p) ? { value: __await(o[n](v)), done: false } : f ? f(v) : v; } : f; }
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[&AWAIT_HELPER],
    import_name: b"__asyncDelegator",
};

pub(crate) static ASYNC_VALUES_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:asyncValues",
    scoped: false,
    text: br#"var __asyncValues = (this && this.__asyncValues) || function (o) {
    if (!Symbol.asyncIterator) throw new TypeError("Symbol.asyncIterator is not defined.");
    var m = o[Symbol.asyncIterator], i;
    return m ? m.call(o) : (o = typeof __values === "function" ? __values(o) : o[Symbol.iterator](), i = {}, verb("next"), verb("throw"), verb("return"), i[Symbol.asyncIterator] = function () { return this; }, i);
    function verb(n) { i[n] = o[n] && function (v) { return new Promise(function (resolve, reject) { v = o[n](v), settle(resolve, reject, v.done, v.value); }); }; }
    function settle(resolve, reject, d, v) { Promise.resolve(v).then(function(v) { resolve({ value: v, done: d }); }, reject); }
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__asyncValues",
};

pub(crate) static REST_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:rest",
    scoped: false,
    text: br#"var __rest = (this && this.__rest) || function (s, e) {
    var t = {};
    for (var p in s) if (Object.prototype.hasOwnProperty.call(s, p) && e.indexOf(p) < 0)
        t[p] = s[p];
    if (s != null && typeof Object.getOwnPropertySymbols === "function")
        for (var i = 0, p = Object.getOwnPropertySymbols(s); i < p.length; i++) {
            if (e.indexOf(p[i]) < 0 && Object.prototype.propertyIsEnumerable.call(s, p[i]))
                t[p[i]] = s[p[i]];
        }
    return t;
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__rest",
};

pub(crate) static AWAITER_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:awaiter",
    scoped: false,
    text: br#"var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    function adopt(value) { return value instanceof P ? value : new P(function (resolve) { resolve(value); }); }
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};"#,
    text_callback: None,
    priority: Some(Priority { value: 5 }),
    dependencies: &[],
    import_name: b"__awaiter",
};

pub static ASYNC_SUPER_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:async-super",
    scoped: true,
    text: b"",
    text_callback: Some(async_super_helper_text),
    priority: None,
    dependencies: &[],
    import_name: b"",
};

pub static ADVANCED_ASYNC_SUPER_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:advanced-async-super",
    scoped: true,
    text: b"",
    text_callback: Some(advanced_async_super_helper_text),
    priority: None,
    dependencies: &[],
    import_name: b"",
};

pub(crate) static ES_DECORATE_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:esDecorate",
    scoped: false,
    text: br#"var __esDecorate = (this && this.__esDecorate) || function (ctor, descriptorIn, decorators, contextIn, initializers, extraInitializers) {
    function accept(f) { if (f !== void 0 && typeof f !== "function") throw new TypeError("Function expected"); return f; }
    var kind = contextIn.kind, key = kind === "getter" ? "get" : kind === "setter" ? "set" : "value";
    var target = !descriptorIn && ctor ? contextIn["static"] ? ctor : ctor.prototype : null;
    var descriptor = descriptorIn || (target ? Object.getOwnPropertyDescriptor(target, contextIn.name) : {});
    var _, done = false;
    for (var i = decorators.length - 1; i >= 0; i--) {
        var context = {};
        for (var p in contextIn) context[p] = p === "access" ? {} : contextIn[p];
        for (var p in contextIn.access) context.access[p] = contextIn.access[p];
        context.addInitializer = function (f) { if (done) throw new TypeError("Cannot add initializers after decoration has completed"); extraInitializers.push(accept(f || null)); };
        var result = (0, decorators[i])(kind === "accessor" ? { get: descriptor.get, set: descriptor.set } : descriptor[key], context);
        if (kind === "accessor") {
            if (result === void 0) continue;
            if (result === null || typeof result !== "object") throw new TypeError("Object expected");
            if (_ = accept(result.get)) descriptor.get = _;
            if (_ = accept(result.set)) descriptor.set = _;
            if (_ = accept(result.init)) initializers.unshift(_);
        }
        else if (_ = accept(result)) {
            if (kind === "field") initializers.unshift(_);
            else descriptor[key] = _;
        }
    }
    if (target) Object.defineProperty(target, contextIn.name, descriptor);
    done = true;
};"#,
    text_callback: None,
    priority: Some(Priority { value: 2 }),
    dependencies: &[],
    import_name: b"__esDecorate",
};

pub(crate) static RUN_INITIALIZERS_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:runInitializers",
    scoped: false,
    text: br"var __runInitializers = (this && this.__runInitializers) || function (thisArg, initializers, value) {
    var useValue = arguments.length > 2;
    for (var i = 0; i < initializers.length; i++) {
        value = useValue ? initializers[i].call(thisArg, value) : initializers[i].call(thisArg);
    }
    return useValue ? value : void 0;
};",
    text_callback: None,
    priority: Some(Priority { value: 2 }),
    dependencies: &[],
    import_name: b"__runInitializers",
};

pub(crate) static MAKE_TEMPLATE_OBJECT_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:makeTemplateObject",
    scoped: false,
    text: br#"var __makeTemplateObject = (this && this.__makeTemplateObject) || function (cooked, raw) {
    if (Object.defineProperty) { Object.defineProperty(cooked, "raw", { value: raw }); } else { cooked.raw = raw; }
    return cooked;
};"#,
    text_callback: None,
    priority: Some(Priority { value: 0 }),
    dependencies: &[],
    import_name: b"__makeTemplateObject",
};

pub(crate) static PROP_KEY_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:propKey",
    scoped: false,
    text: br#"var __propKey = (this && this.__propKey) || function (x) {
    return typeof x === "symbol" ? x : "".concat(x);
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__propKey",
};

pub(crate) static SET_FUNCTION_NAME_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:setFunctionName",
    scoped: false,
    text: br#"var __setFunctionName = (this && this.__setFunctionName) || function (f, name, prefix) {
    if (typeof name === "symbol") name = name.description ? "[".concat(name.description, "]") : "";
    return Object.defineProperty(f, "name", { configurable: true, value: prefix ? "".concat(prefix, " ", name) : name });
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__setFunctionName",
};

pub(crate) static CREATE_BINDING_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:commonjscreatebinding",
    scoped: false,
    text: br#"var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));"#,
    text_callback: None,
    priority: Some(Priority { value: 1 }),
    dependencies: &[],
    import_name: b"__createBinding",
};

pub(crate) static SET_MODULE_DEFAULT_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:commonjscreatevalue",
    scoped: false,
    text: br#"var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});"#,
    text_callback: None,
    priority: Some(Priority { value: 1 }),
    dependencies: &[],
    import_name: b"__setModuleDefault",
};

pub(crate) static IMPORT_STAR_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:commonjsimportstar",
    scoped: false,
    text: br#"var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();"#,
    text_callback: None,
    priority: Some(Priority { value: 2 }),
    dependencies: &[&CREATE_BINDING_HELPER, &SET_MODULE_DEFAULT_HELPER],
    import_name: b"__importStar",
};

pub(crate) static IMPORT_DEFAULT_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:commonjsimportdefault",
    scoped: false,
    text: br#"var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__importDefault",
};

pub(crate) static EXPORT_STAR_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:export-star",
    scoped: false,
    text: br#"var __exportStar = (this && this.__exportStar) || function(m, exports) {
    for (var p in m) if (p !== "default" && !Object.prototype.hasOwnProperty.call(exports, p)) __createBinding(exports, m, p);
};"#,
    text_callback: None,
    priority: Some(Priority { value: 2 }),
    dependencies: &[&CREATE_BINDING_HELPER],
    import_name: b"__exportStar",
};

pub(crate) static REWRITE_RELATIVE_IMPORT_EXTENSIONS_HELPER: EmitHelper = EmitHelper {
    name: b"typescript:rewriteRelativeImportExtensions",
    scoped: false,
    text: br#"var __rewriteRelativeImportExtension = (this && this.__rewriteRelativeImportExtension) || function (path, preserveJsx) {
    if (typeof path === "string" && /^\.\.?\//.test(path)) {
        return path.replace(/\.(tsx)$|((?:\.d)?)((?:\.[^./]+?)?)\.([cm]?)ts$/i, function (m, tsx, d, ext, cm) {
            return tsx ? preserveJsx ? ".jsx" : ".js" : d && (!ext || !cm) ? m : (d + ext + "." + cm.toLowerCase() + "js");
        });
    }
    return path;
};"#,
    text_callback: None,
    priority: None,
    dependencies: &[],
    import_name: b"__rewriteRelativeImportExtension",
};

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    // Expected values were produced by compiling the pinned helpers.go into a
    // Go program that printed each helper's fields, text length and FNV-1a
    // hash, and each callback's text with makeUniqueName wrapping its
    // argument in angle brackets.
    type Expected<'a> = (
        &'a EmitHelper,
        &'a [u8],
        &'a [u8],
        bool,
        Option<i64>,
        &'a [&'a EmitHelper],
        usize,
        u64,
    );

    #[test]
    fn emit_helper_texts_match_the_pinned_go() {
        let expected: [Expected<'_>; 27] = [
            (
                &DECORATE_HELPER,
                b"typescript:decorate",
                b"__decorate",
                false,
                Some(2),
                &[],
                571,
                0xd44b_d82c_1854_7ccb,
            ),
            (
                &METADATA_HELPER,
                b"typescript:metadata",
                b"__metadata",
                false,
                Some(3),
                &[],
                176,
                0x166f_5d0b_2d6b_6a88,
            ),
            (
                &PARAM_HELPER,
                b"typescript:param",
                b"__param",
                false,
                Some(4),
                &[],
                151,
                0x0720_aa83_6cc2_2bb3,
            ),
            (
                &ADD_DISPOSABLE_RESOURCE_HELPER,
                b"typescript:addDisposableResource",
                b"__addDisposableResource",
                false,
                None,
                &[],
                1054,
                0xe2a9_2ca1_9607_a8c0,
            ),
            (
                &DISPOSE_RESOURCES_HELPER,
                b"typescript:disposeResources",
                b"__disposeResources",
                false,
                None,
                &[],
                1325,
                0x790a_b029_ec2c_0d5a,
            ),
            (
                &CLASS_PRIVATE_FIELD_GET_HELPER,
                b"typescript:classPrivateFieldGet",
                b"__classPrivateFieldGet",
                false,
                None,
                &[],
                491,
                0xe8ac_9b7f_c549_00fe,
            ),
            (
                &CLASS_PRIVATE_FIELD_SET_HELPER,
                b"typescript:classPrivateFieldSet",
                b"__classPrivateFieldSet",
                false,
                None,
                &[],
                586,
                0xee52_781f_78f6_489d,
            ),
            (
                &CLASS_PRIVATE_FIELD_IN_HELPER,
                b"typescript:classPrivateFieldIn",
                b"__classPrivateFieldIn",
                false,
                None,
                &[],
                339,
                0xfab0_293e_e6ba_ac8d,
            ),
            (
                &AWAIT_HELPER,
                b"typescript:await",
                b"__await",
                false,
                None,
                &[],
                126,
                0x0ff7_5d42_622c_3bf4,
            ),
            (
                &ASYNC_GENERATOR_HELPER,
                b"typescript:asyncGenerator",
                b"__asyncGenerator",
                false,
                None,
                &[&AWAIT_HELPER],
                1166,
                0xd4ad_4623_c94b_ad83,
            ),
            (
                &ASYNC_DELEGATOR_HELPER,
                b"typescript:asyncDelegator",
                b"__asyncDelegator",
                false,
                None,
                &[&AWAIT_HELPER],
                373,
                0x6f97_4e56_d79c_8eb3,
            ),
            (
                &ASYNC_VALUES_HELPER,
                b"typescript:asyncValues",
                b"__asyncValues",
                false,
                None,
                &[],
                709,
                0x1b3e_53c1_207a_a21d,
            ),
            (
                &REST_HELPER,
                b"typescript:rest",
                b"__rest",
                false,
                None,
                &[],
                490,
                0x253a_389c_11a5_ec09,
            ),
            (
                &AWAITER_HELPER,
                b"typescript:awaiter",
                b"__awaiter",
                false,
                Some(5),
                &[],
                680,
                0xeb54_b696_d748_e349,
            ),
            (
                &ASYNC_SUPER_HELPER,
                b"typescript:async-super",
                b"",
                true,
                None,
                &[],
                0,
                0xcbf2_9ce4_8422_2325,
            ),
            (
                &ADVANCED_ASYNC_SUPER_HELPER,
                b"typescript:advanced-async-super",
                b"",
                true,
                None,
                &[],
                0,
                0xcbf2_9ce4_8422_2325,
            ),
            (
                &ES_DECORATE_HELPER,
                b"typescript:esDecorate",
                b"__esDecorate",
                false,
                Some(2),
                &[],
                1780,
                0x990e_fc23_712f_129b,
            ),
            (
                &RUN_INITIALIZERS_HELPER,
                b"typescript:runInitializers",
                b"__runInitializers",
                false,
                Some(2),
                &[],
                338,
                0xacc0_78e8_a925_1d5b,
            ),
            (
                &MAKE_TEMPLATE_OBJECT_HELPER,
                b"typescript:makeTemplateObject",
                b"__makeTemplateObject",
                false,
                Some(0),
                &[],
                228,
                0xdae9_61ca_882d_b219,
            ),
            (
                &PROP_KEY_HELPER,
                b"typescript:propKey",
                b"__propKey",
                false,
                None,
                &[],
                114,
                0x67ab_18ac_c5e6_80ed,
            ),
            (
                &SET_FUNCTION_NAME_HELPER,
                b"typescript:setFunctionName",
                b"__setFunctionName",
                false,
                None,
                &[],
                313,
                0xae4b_04ae_3904_adab,
            ),
            (
                &CREATE_BINDING_HELPER,
                b"typescript:commonjscreatebinding",
                b"__createBinding",
                false,
                Some(1),
                &[],
                476,
                0xb065_33d9_d9e8_af06,
            ),
            (
                &SET_MODULE_DEFAULT_HELPER,
                b"typescript:commonjscreatevalue",
                b"__setModuleDefault",
                false,
                Some(1),
                &[],
                217,
                0xd33e_b70a_4eee_deef,
            ),
            (
                &IMPORT_STAR_HELPER,
                b"typescript:commonjsimportstar",
                b"__importStar",
                false,
                Some(2),
                &[&CREATE_BINDING_HELPER, &SET_MODULE_DEFAULT_HELPER],
                663,
                0x6d0f_4efb_ec5f_7ca3,
            ),
            (
                &IMPORT_DEFAULT_HELPER,
                b"typescript:commonjsimportdefault",
                b"__importDefault",
                false,
                None,
                &[],
                138,
                0x2524_e644_8ab8_6666,
            ),
            (
                &EXPORT_STAR_HELPER,
                b"typescript:export-star",
                b"__exportStar",
                false,
                Some(2),
                &[&CREATE_BINDING_HELPER],
                202,
                0xe471_8b01_ba56_afa5,
            ),
            (
                &REWRITE_RELATIVE_IMPORT_EXTENSIONS_HELPER,
                b"typescript:rewriteRelativeImportExtensions",
                b"__rewriteRelativeImportExtension",
                false,
                None,
                &[],
                455,
                0x139e_9c27_2fd7_8891,
            ),
        ];
        let mut total = 0;
        for (helper, name, import_name, scoped, priority, dependencies, len, hash) in expected {
            assert_eq!(helper.name, name);
            assert_eq!(helper.import_name, import_name);
            assert_eq!(helper.scoped, scoped);
            assert_eq!(helper.priority.map(|p| p.value), priority);
            assert_eq!(helper.dependencies, dependencies);
            assert_eq!(helper.text.len(), len, "{:?}", helper);
            assert_eq!(fnv1a64(helper.text), hash, "{:?}", helper);
            total += helper.text.len();
        }
        assert_eq!(total, 13161);
        let mut wrap = |name: &[u8]| [b"<".as_slice(), name, b">"].concat();
        assert_eq!(
            (ASYNC_SUPER_HELPER.text_callback.unwrap())(&mut wrap),
            b"\nconst <_superIndex> = name => super[name];".to_vec()
        );
        let mut wrap = |name: &[u8]| [b"<".as_slice(), name, b">"].concat();
        assert_eq!(
            (ADVANCED_ASYNC_SUPER_HELPER.text_callback.unwrap())(&mut wrap),
            b"\nconst <_superIndex> = (function (geti, seti) {\n    const cache = Object.create(null);\n    return name => cache[name] || (cache[name] = { get value() { return geti(name); }, set value(v) { seti(name, v); } });\n})(name => super[name], (name, value) => super[name] = value);".to_vec()
        );
    }
}

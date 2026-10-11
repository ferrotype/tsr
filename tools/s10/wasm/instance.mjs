// The generated bindings live in a factory, not an ES module singleton. Each
// call gets its own wasm globals and can release its instance on dispose/trap.
export class WasmApiError extends Error {
  constructor(message) { super(message); this.name = "WasmApiError"; }
}
const API_ERROR = "ts-wasm-api-error:";

export function createInstance(createBindings, module) {
  const bindings = createBindings();
  bindings.initSync({ module });
  return manageInstance(bindings);
}

// Browser main threads limit synchronous instantiation of large modules.
// Keep the same ownership/trap controller after the generated async initializer.
export async function createInstanceAsync(createBindings, module) {
  const bindings = createBindings();
  await bindings({ module_or_path: module });
  return manageInstance(bindings);
}

function manageInstance(bindings) {
  let state = "live";
  const resources = new Set();
  function invoke(operation) {
    if (state !== "live") throw new Error(`WebAssembly instance is ${state}`);
    try {
      return operation();
    } catch (error) {
      if (typeof error === "string" && error.startsWith(API_ERROR)) {
        // Rust returned normally with Err, releasing its operation scope.
        // A retired session stays retired; independent sessions remain usable.
        throw new WasmApiError(error.slice(API_ERROR.length));
      }
      // Rust aborts do not run destructors. Never reenter a possibly poisoned
      // instance. Owned output copies from earlier successful calls stay valid.
      bindings = null;
      state = "failed";
      for (const release of resources) release(false, true);
      resources.clear();
      throw error;
    }
  }
  function call(name, args) { return invoke(() => bindings[name](...args)); }
  function owned(value) {
    let raw = value;
    const release = (runDestructor, detachFinalizer = false) => {
      const old = raw;
      raw = null;
      resources.delete(release);
      // Pinned wasm-bindgen's JS-only detach clears its pointer and
      // unregisters FinalizationRegistry without calling back into wasm.
      if (detachFinalizer && old !== null) old.__destroy_into_raw();
      if (runDestructor && old !== null) old.free();
    };
    resources.add(release);
    return {
      use(operation) {
        if (raw === null) throw new Error("WebAssembly object is disposed");
        return invoke(() => operation(raw));
      },
      consume(operation) {
        if (raw === null) throw new Error("WebAssembly object is disposed");
        return invoke(() => {
          const old = raw;
          release(false);
          return operation(old);
        });
      },
      dispose() {
        if (raw !== null) invoke(() => release(true));
      },
    };
  }
  function session(value) {
    const object = owned(value);
    return Object.freeze({
      diagnostics: () => JSON.parse(new TextDecoder().decode(object.use(raw => raw.diagnostics()))),
      resolutionTrace: () => JSON.parse(new TextDecoder().decode(object.use(raw => raw.resolution_trace()))),
      typeAtPosition(path, position) {
        if (!(path instanceof Uint8Array) || !Number.isInteger(position) || position < 0 || position > 0xffffffff) {
          throw new TypeError("type query requires a byte path and unsigned UTF-16 position");
        }
        return object.use(raw => raw.type_at_position(path, position));
      },
      emit(request = {}) {
        // Byte paths travel as JSON byte arrays; Rust validates every field.
        if (request === null || typeof request !== "object" || Array.isArray(request)) {
          throw new TypeError("emit request must be an object");
        }
        const { files, ...rest } = request;
        if (files !== undefined && files !== null &&
            (!Array.isArray(files) || !files.every(file => file instanceof Uint8Array))) {
          throw new TypeError("emit files must be an array of byte paths");
        }
        const json = JSON.stringify(files == null ? rest : { ...rest, files: files.map(file => Array.from(file)) });
        if (json === undefined) throw new TypeError("emit request must be JSON");
        const result = JSON.parse(new TextDecoder().decode(object.use(raw => raw.emit(json))));
        for (const file of result.files) {
          file.name = Uint8Array.from(file.name);
          file.text = Uint8Array.from(file.text);
        }
        return result;
      },
      retire: () => object.use(raw => raw.retire()),
      dispose: () => object.dispose(),
    });
  }
  function sourceArgs(source, name, kind, jsx = false, force = false) {
    if (!(source instanceof Uint8Array) || !(name instanceof Uint8Array)) {
      throw new TypeError("source and filename must be Uint8Array values");
    }
    if (!Number.isInteger(kind) || kind < -2147483648 || kind > 2147483647) {
      throw new RangeError("script kind must fit a signed 32-bit integer");
    }
    if (source.length > 2147483647 || name.length > 2147483647) {
      throw new RangeError("source and filename must fit signed source positions");
    }
    if (typeof jsx !== "boolean" || typeof force !== "boolean") {
      throw new TypeError("parse options must be boolean");
    }
    return [source, name, kind, jsx, force];
  }
  function transpileArgs(source, name, optionsJSON, reportDiagnostics) {
    sourceArgs(source, name, 0);
    if (typeof optionsJSON !== "string" || typeof reportDiagnostics !== "boolean") {
      throw new TypeError("transpile requires raw compiler-options JSON and a diagnostic boolean");
    }
    return [source, name, optionsJSON, reportDiagnostics];
  }
  return Object.freeze({
    parse(source, name, kind, jsx = false, force = false) { return call("parse", sourceArgs(source, name, kind, jsx, force)); },
    parseAndEncode(source, name, kind, jsx = false, force = false) {
      return call("parse_and_encode", sourceArgs(source, name, kind, jsx, force));
    },
    transpileModule(source, name, optionsJSON, reportDiagnostics = false) {
      return call("transpile_module", transpileArgs(source, name, optionsJSON, reportDiagnostics));
    },
    transpileDeclaration(source, name, optionsJSON, reportDiagnostics = false) {
      return call("transpile_declaration", transpileArgs(source, name, optionsJSON, reportDiagnostics));
    },
    createHost(cwd, caseSensitive = true) {
      if (!(cwd instanceof Uint8Array) || typeof caseSensitive !== "boolean") {
        throw new TypeError("host requires a byte directory and boolean case sensitivity");
      }
      const object = owned(invoke(() => new bindings.MemoryHost(cwd, caseSensitive)));
      return Object.freeze({
        addFile(path, contents, root = false) {
          sourceArgs(contents, path, 0);
          if (typeof root !== "boolean") throw new TypeError("root must be boolean");
          object.use(raw => raw.add_file(path, contents, root));
        },
        addDirectory(path) {
          sourceArgs(new Uint8Array(), path, 0);
          object.use(raw => raw.add_directory(path));
        },
        addSymlink(path, target) {
          sourceArgs(target, path, 0);
          object.use(raw => raw.add_symlink(path, target));
        },
        compile(options) {
          // Serialization can fail before Rust consumes the host. Keep the
          // host and its finalizer registered on these JS validation errors.
          const json = JSON.stringify(options);
          if (json === undefined) throw new TypeError("compiler options must be JSON");
          return session(object.consume(raw => raw.compile(json)));
        },
        dispose: () => object.dispose(),
      });
    },
    observeCorpus(request) { return JSON.parse(new TextDecoder().decode(call("observe_corpus", [JSON.stringify(request)]))); },
    observeCorpusJSON(request) {
      if (typeof request !== "string") throw new TypeError("corpus request must be JSON text");
      return new TextDecoder().decode(call("observe_corpus", [request]));
    },
    dispose() {
      if (state === "live") {
        invoke(() => { for (const release of resources) release(true); });
        bindings = null;
        state = "disposed";
      }
    },
    get state() { return state; },
  });
}

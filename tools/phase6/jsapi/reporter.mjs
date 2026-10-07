// A Node test reporter that writes one JSON line per test event the suite
// adapter reads: starts, passes, failures and skips with their nesting, file
// and error text. Nothing else goes to stdout; the pinned tests keep their own
// stdout/stderr events, which are passed through as `output` lines.
export default async function* reporter(source) {
    for await (const event of source) {
        const data = event.data ?? {};
        switch (event.type) {
            case "test:start":
            case "test:pass":
            case "test:fail": {
                const details = data.details ?? {};
                let error;
                if (details.error) {
                    error = describeError(details.error);
                }
                yield JSON.stringify({
                    type: event.type,
                    name: data.name,
                    nesting: data.nesting,
                    file: data.file,
                    line: data.line,
                    column: data.column,
                    kind: details.type,
                    skip: data.skip === undefined ? undefined : String(data.skip),
                    todo: data.todo === undefined ? undefined : String(data.todo),
                    durationMs: details.duration_ms,
                    error,
                }) + "\n";
                break;
            }
            case "test:stdout":
            case "test:stderr":
                yield JSON.stringify({ type: "output", stream: event.type.slice(5), message: data.message, file: data.file }) + "\n";
                break;
            case "test:diagnostic":
                yield JSON.stringify({ type: "diagnostic", message: data.message, file: data.file, nesting: data.nesting }) + "\n";
                break;
            default:
                break;
        }
    }
}

function describeError(error) {
    const parts = [];
    let current = error;
    for (let depth = 0; current && depth < 8; depth++) {
        const text = current.stack ?? current.message ?? String(current);
        parts.push(depth === 0 ? text : `caused by: ${text}`);
        if (current.code !== undefined) parts.push(`code: ${current.code}`);
        current = current.cause;
    }
    return parts.join("\n");
}

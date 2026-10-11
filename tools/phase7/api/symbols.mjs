// Symbol IDs are process-local (ADR 0010). Go also embeds a type ID in the
// escaped name of unique-symbol properties. Renumber those IDs by encounter
// order within one task, retaining identity/equality and the property's name.
// User-written "__@..." names have a third escaped underscore, so do not match.
export function symbolObserver(root) {
    const ids = new Map();
    const handle = value => value ? [value.path.replaceAll(root, "@UPSTREAM@/"), value.kind, value.index] : null;
    return value => {
        if (!value) return null;
        let name = value.name;
        let escapedName = value.escapedName;
        const match = /^(__@.*)@(\d+)$/.exec(escapedName);
        if (match) {
            if (!ids.has(match[2])) ids.set(match[2], ids.size);
            const normalized = `${match[1]}@id:${ids.get(match[2])}`;
            if (name === escapedName) name = normalized;
            escapedName = normalized;
        }
        return { name, escapedName, flags: value.flags, checkFlags: value.checkFlags,
            declarations: value.declarations.map(handle), valueDeclaration: handle(value.valueDeclaration) };
    };
}

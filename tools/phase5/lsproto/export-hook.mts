// Appended inside the pinned module. Every type, field order and discriminator
// below comes from that resolver; the Rust emitter only spells Rust syntax.
function exportFallback(entries: any[]): any {
    if (entries.length === 1) return { kind: 'direct', field: entries[0].fieldName };
    const pres = findPresenceDiscriminator(entries);
    if (!pres) return { kind: 'try', fields: entries.map(e => e.fieldName) };
    const checks = [...pres.checks];
    let remaining = pres.unmapped;
    while (remaining.length > 1) {
        const next = findPresenceDiscriminator(remaining);
        if (!next) break;
        checks.push(...next.checks);
        remaining = next.unmapped;
    }
    return {
        kind: 'presence', checks: checks.map(c => ({ name: c.jsonFieldName, field: c.entry.fieldName })),
        fallback: remaining.length === 1 ? { kind: 'direct', field: remaining[0].fieldName }
            : { kind: 'try', fields: remaining.map(e => e.fieldName) },
    };
}
function exportDispatch(entries: any[]): any {
    const disc = findDiscriminatorField(entries);
    if (disc) return {
        kind: 'discriminator', name: disc.fieldName,
        cases: [...disc.mapping].map(([value, entry]) => ({ value, field: entry.fieldName })),
        fallback: exportFallback(disc.unmapped),
    };
    // Unlike the remaining-discriminator path, a single initial member in a
    // buffered union is speculative. PeekKind's single-member groups stream.
    const pres = findPresenceDiscriminator(entries);
    return pres ? exportFallback(entries) : { kind: 'try', fields: entries.map(e => e.fieldName) };
}
async function exportModel() {
    collectTypeDefinitions();
    const go = generateCode(); // Run the original validations and discovery.
    if (process.argv[4]) fs.writeFileSync(process.argv[4], go);
    const describe = (t: any) => t ? resolveType(t) : null;
    const schema = {
        version: 1, pin: process.argv[3], modelVersion: model.metaData.version,
        structures: model.structures.map(s => ({ name: s.name, fields: exportedStructures.get(s.name) ?? [],
            strict: s.name !== 'Registration' && s.properties.some(p => !p.optional && !p.omitzeroValue
                || !p.omitzeroValue && !typeCanBeNull(p.type) && (p.optional || resolveType(p.type).needsPointer
                    || resolveType(p.type).name.startsWith('[]') || resolveType(p.type).name.startsWith('map['))) })),
        enumerations: model.enumerations.map(e => ({ name: e.name, type: resolveType(e.type).name,
            values: e.values, bitflags: exportedBitflags.has(e.name) })),
        unions: [...exportedUnions.values()].map(u => ({ ...u,
            members: u.members.map(m => ({ name: m.fieldName, type: m.typeName })),
            groups: u.groups.map(g => ({ kind: g.kind, fields: g.entries.map(e => e.fieldName), plan: g.plan })),
        })),
        literals: [...typeInfo.literalTypes].map(([value, name]) => ({ name, json: JSON.stringify(value) })),
        aliases: customTypeAliases.map(a => ({ name: a.name, target: resolveType(a.type) })),
        methods: [...model.requests, ...model.notifications].map(m => ({ method: m.method, name: methodNameIdentifier(m.method),
            params: describe(m.params), result: 'result' in m ? describe(m.result) : null,
            nullResult: 'result' in m && m.result?.kind === 'base' && m.result.name === 'null', request: 'result' in m })),
        registrations: registrationMethods,
    };
    fs.writeFileSync(process.argv[2], JSON.stringify(schema, null, 2) + '\n');
}

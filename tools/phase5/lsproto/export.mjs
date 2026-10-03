// Access-only adapter: run the pinned resolver, export the facts its Go emitter uses.
// No npm install, network request, or write into canonical upstream/.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../../..');
const output = process.argv[2];
if (!output) throw new Error('usage: node tools/phase5/lsproto/export.mjs OUTPUT');
const upstream = path.join(root, 'upstream');
const pin = JSON.parse(fs.readFileSync(path.join(root, 'data/upstream.json'))).pin;
if (execFileSync('git', ['rev-parse', 'HEAD'], { cwd: upstream, encoding: 'utf8' }).trim() !== pin) {
    throw new Error('initialize upstream at the pinned revision before generating LSP types');
}
const readPinned = name => execFileSync('git', ['show', `${pin}:${name}`], { cwd: upstream, maxBuffer: 16 * 1024 * 1024 });
const nodePin = JSON.parse(readPinned('package.json')).volta.node;
if (process.version !== `v${nodePin}`) throw new Error(`LSP generation requires node ${nodePin}; found ${process.version}`);
const version = JSON.parse(readPinned('package-lock.json')).packages['node_modules/vscode-languageclient'].version;
const source = JSON.parse(fs.readFileSync(path.join(here, 'model-source.json')));
const model = fs.readFileSync(path.join(here, 'metaModel.json'));
const expectedUrl = `https://raw.githubusercontent.com/microsoft/vscode-languageserver-node/release/client/${version}/protocol/metaModel.json`;
if (source.client_version !== version || source.url !== expectedUrl || source.sha256 !== crypto.createHash('sha256').update(model).digest('hex')) {
    throw new Error('vendored LSP metamodel differs from its pinned source');
}
let generator = readPinned('tsc/internal/lsp/lsproto/_generate/generate.mts').toString().replaceAll('\r\n', '\n');
function replaceOnce(before, after) {
    if (generator.split(before).length !== 2) throw new Error(`pinned LSP generator hook drift: ${before}`);
    generator = generator.replace(before, after);
}
replaceOnce('import { x } from "tinyexec";', ''); // The formatter is not run by this export-only entry point.
replaceOnce('const typeInfo: TypeInfo = {', 'const exportedStructures = new Map();\nconst exportedUnions = new Map();\nconst exportedBitflags = new Set();\nconst typeInfo: TypeInfo = {');
replaceOnce('const isBitflag = isBitflagEnum(enumeration);', 'const isBitflag = isBitflagEnum(enumeration);\n            if (isBitflag) exportedBitflags.add(enumeration.name);');
replaceOnce('const lspTag = lspMarkers ? ` lsp:"${lspMarkers}"` : "";', `const lspTag = lspMarkers ? \` lsp:"\${lspMarkers}"\` : "";
                if (includeDocumentation) {
                    if (!exportedStructures.has(structure.name)) exportedStructures.set(structure.name, []);
                    exportedStructures.get(structure.name).push({
                        name: prop.name, goType, required, nullable, nilable, omitZero: !!useOmitzero,
                        rejectNull: !nullable && (goType.startsWith('*') || goType.startsWith('[]') || goType.startsWith('map[')),
                    });
                }`);
replaceOnce('const canDispatch = !hasUnknownKinds && distinctKinds >= 2;', `const canDispatch = !hasUnknownKinds && distinctKinds >= 2;
        exportedUnions.set(name, {
            name, nullable: unionContainedNull, members: fieldEntries,
            dispatch: canDispatch,
            groups: [...kindMap].map(([kind, entries]) => ({ kind, entries, plan: exportDispatch(entries) })),
            fallback: exportDispatch(fieldEntries),
        });`);
replaceOnce('main().catch(e => {', 'exportModel().catch(e => {');
generator += '\n' + fs.readFileSync(path.join(here, 'export-hook.mts'), 'utf8');
const stage = fs.mkdtempSync(path.join(os.tmpdir(), 'tsr-lsproto-'));
try {
    fs.writeFileSync(path.join(stage, 'generate.mts'), generator);
    fs.writeFileSync(path.join(stage, 'metaModel.json'), model);
    fs.writeFileSync(path.join(stage, 'metaModelSchema.mts'), readPinned('tsc/internal/lsp/lsproto/_generate/metaModelSchema.mts'));
    const args = [path.join(stage, 'generate.mts'), path.resolve(output), pin];
    if (process.argv[3]) args.push(path.resolve(process.argv[3]));
    execFileSync(process.execPath, args, { stdio: 'inherit' });
} finally {
    fs.rmSync(stage, { recursive: true, force: true });
}

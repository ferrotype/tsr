#!/usr/bin/env python3
"""Generate the checked-in, reviewable bounded fixtures and sessions (no network)."""
import json
from pathlib import Path
HOME = Path(__file__).resolve().parent


def write(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)


def generate():
    for name in ('references', 'checkjs', 'monorepo'):
        base = HOME / 'fixtures' / name
        js = name == 'checkjs'
        extension = 'js' if js else 'ts'
        options = {'strict': True, 'noLib': True, 'module': 'nodenext', 'moduleResolution': 'nodenext'}
        if js:
            options.update(allowJs=True, checkJs=True)
        write(base / 'tsconfig.json', json.dumps({'compilerOptions': options, 'typeAcquisition': {'enable': False}, 'include': ['src/**/*']}, indent=2) + '\n')
        for index in range(20):
            text = f'export const value{index} = {index};\n'
            if js:
                text = f'/** @type {{number}} */\nexport const value{index} = {index};\n'
            write(base / (f'core/value{index}.ts' if name == 'references' else f'src/value{index}.{extension}'), text)
        text = 'import { value0 } from "./value0.js";\nexport const answer = value0 + 1;\nanswer;\n'
        if name != 'references':
            write(base / f'src/main.{extension}', text)
        if js:
            write(base / 'node_modules/local-commonjs/package.json', '{"name":"local-commonjs","main":"index.js"}\n')
            write(base / 'node_modules/local-commonjs/index.js', '/** @param {number} x */\nexports.double = x => x * 2;\n')
            write(base / 'src/dependency.js', 'const { double } = require("local-commonjs");\n/** @type {number} */\nconst result = double(2);\nmodule.exports = result;\n')
        if name == 'references':
            write(base / 'tsconfig.json', '{"files":[],"references":[{"path":"./core"},{"path":"./middle"},{"path":"./app"}],"typeAcquisition":{"enable":false}}\n')
            for project, refs, source in [('core', [], ''.join(f'export * from "./value{i}.js";\n' for i in range(20)) + 'export { value0 as shared } from "./value0.js";\n'), ('middle', ['../core'], 'export { shared } from "../core/index.js";\n'), ('app', ['../middle'], 'import { shared } from "../middle/index.js";\nexport const answer = shared;\nanswer;\n')]:
                write(base / project / 'tsconfig.json', json.dumps({'compilerOptions': {**options, 'composite': True, 'declaration': True, 'declarationMap': True}, 'files': ['index.ts', *([f'value{i}.ts' for i in range(20)] if project == 'core' else [])], 'references': [{'path': ref} for ref in refs], 'typeAcquisition': {'enable': False}}, indent=2) + '\n')
                write(base / project / 'index.ts', source)
            document = 'app/index.ts'
            text = (base / document).read_text()
        elif name == 'monorepo':
            write(base / 'packages/lib/package.json', '{"name":"local-lib","type":"module","exports":{".":{"types":"./index.d.ts","import":"./index.js","require":"./index.cjs"}}}\n')
            for file, contents in [('index.d.ts', 'export declare const shared: number;\n'), ('index.js', 'export const shared = 1;\n'), ('index.cjs', 'exports.shared = 1;\n')]:
                write(base / 'packages/lib' / file, contents)
            link = base / 'node_modules/local-lib'
            link.parent.mkdir(parents=True, exist_ok=True)
            if not link.is_symlink():
                link.symlink_to('../packages/lib', target_is_directory=True)
            write(base / 'node_modules/@types/local/index.d.ts', 'declare const localGlobal: number;\n')
            write(base / 'src/package.ts', 'import { shared } from "local-lib";\nexport const packageValue = shared + localGlobal;\n')
            document = f'src/main.{extension}'
        else:
            document = f'src/main.{extension}'
        text = text.replace('\nanswer;\n', '\n/*😀*/ answer;\n')
        text += 'export const box = { value: answer };\nbox.value;\n'
        write(base / document, text)
        uri = '@PROJECT_ROOT_URI@/' + document
        query = {'textDocument': {'uri': uri}, 'position': {'$position': 'symbol'}}
        steps = []
        def send(kind, method, params=None):
            step = {'kind': kind, 'method': method}
            if params is not None:
                step['params'] = params
            steps.append(step)
        send('request', 'initialize', {'processId': None, 'initializationOptions': {'logVerbosity': 5}, 'rootUri': '@PROJECT_ROOT_URI@', 'capabilities': {'general': {'positionEncodings': ['@ENCODING@']}, 'textDocument': {'diagnostic': {}, 'completion': {'completionItem': {'resolveSupport': {'properties': ['documentation', 'detail']}}}}, 'workspace': {'configuration': True}}})
        send('notification', 'initialized', {})
        send('notification', 'textDocument/didOpen', {'textDocument': {'uri': uri, 'languageId': 'javascript' if js else 'typescript', 'version': 1, 'text': text}})
        send('request', 'textDocument/diagnostic', {'textDocument': {'uri': uri}})
        send('request', 'textDocument/completion', {'textDocument': {'uri': uri}, 'position': {'line': len(text.splitlines()) - 1, 'character': 4}})
        send('request', 'completionItem/resolve', {'$response': 2})
        for method in ('hover', 'definition', 'references', 'rename'):
            params = dict(query)
            if method == 'references':
                params['context'] = {'includeDeclaration': True}
            if method == 'rename':
                params['newName'] = 'renamedAnswer'
            send('request', 'textDocument/' + method, params)
        send('request', 'textDocument/codeAction', {'textDocument': {'uri': uri}, 'range': {'start': query['position'], 'end': query['position']}, 'context': {'diagnostics': [], 'only': ['source.organizeImports']}})
        send('request', 'textDocument/formatting', {'textDocument': {'uri': uri}, 'options': {'tabSize': 2, 'insertSpaces': True}})
        send('notification', 'textDocument/didChange', {'textDocument': {'uri': uri, 'version': 2}, 'contentChanges': [{'text': text + '// edited\n'}]})
        send('notification', 'workspace/didChangeConfiguration', {'settings': {'js/ts': {'validate': {'enabled': False}}}})
        mutation_position = len(steps)
        changed_source = 'core/value0.ts' if name == 'references' else f'src/value0.{extension}'
        send('notification', 'workspace/didChangeWatchedFiles', {'changes': [{'uri': '@PROJECT_ROOT_URI@/tsconfig.json', 'type': 2}, {'uri': '@PROJECT_ROOT_URI@/' + changed_source, 'type': 2}]})
        send('request', 'textDocument/diagnostic', {'textDocument': {'uri': uri}})
        send('notification', 'textDocument/didClose', {'textDocument': {'uri': uri}})
        send('request', 'shutdown')
        send('notification', 'exit')
        write(HOME / 'sessions' / f'{name}.jsonl', '\n'.join(json.dumps(row) for row in [{'fixture': name, 'projectRoot': '@PROJECT_ROOT@', 'projectRootUri': '@PROJECT_ROOT_URI@', 'positions': {'symbol': {'utf-8': {'line': 2, 'character': len('/*😀*/ an'.encode('utf-8'))}, 'utf-16': {'line': 2, 'character': len('/*😀*/ an'.encode('utf-16-le')) // 2}}}, 'mutations': [{'before': mutation_position, 'path': 'tsconfig.json', 'text': json.dumps({**json.loads((base / 'tsconfig.json').read_text()), 'compileOnSave': True}) + '\n'}, {'before': mutation_position, 'path': changed_source, 'text': 'export const value0 = 9;\n'}]}, *steps]) + '\n')

if __name__ == '__main__':
    generate()

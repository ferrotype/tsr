#!/usr/bin/env python3
"""Generate and apply diagnostic ownership walkers to an existing staged tree.

No compiler checkout is modified. The additions are additive: no production
field, layout, parse/bind operation, or allocation site is rewritten.
"""
import argparse
import difflib
import hashlib
import json
from pathlib import Path
import re

from arena_bridges import BRIDGES

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
MARKER = '// Diagnostic retained census: staged copy only.'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def body(source, keyword, name):
    match = re.search(r'\b' + keyword + r'\s+' + re.escape(name) + r'\s*\{', source)
    if not match:
        raise ValueError(f'missing concrete {keyword} {name}')
    start = match.end(); depth = 1; end = start
    while depth:
        if end >= len(source):
            raise ValueError(f'unclosed {name}')
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end-1]


def fields(source, name):
    value = body(source, 'struct', name)
    result = []
    for line in value.splitlines():
        line = line.strip()
        if not line or line.startswith('//'):
            continue
        match = re.fullmatch(r'(?:pub(?:\([^)]*\))?\s+)?((?:r#)?\w+):\s*(.+),', line)
        if not match:
            raise ValueError(f'unrecognized {name} field: {line}')
        result.append(match.groups())
    return result


def implement(name, selected, *, tuple_field=False):
    statements = '\n'.join(
        f'        self.{field}.walk(c, "{name}.{field}");' for field in selected)
    parameter = 'c' if selected else '_c'
    return f'''
impl tsr_jsstring::census::Walk for {name} {{
    fn walk(&self,{parameter}:&mut tsr_jsstring::census::Collector,_:&str) {{
{statements}
    }}
}}
'''


def enum_walk(source, name):
    variants = []
    for line in body(source, 'enum', name).splitlines():
        line = line.strip()
        if not line or line.startswith('//'):
            continue
        match = re.fullmatch(r'(\w+)\((.+)\),', line)
        if not match:
            raise ValueError(f'unrecognized {name} variant: {line}')
        variants.append(match.groups())
    arms = '\n'.join(f'            Self::{variant}(value) => {{ c.record("{name}.{variant}.records",0,0,1,1,0); value.walk(c,"{name}.{variant}.owned"); }},'
                     for variant, _ in variants)
    return f'''
impl tsr_jsstring::census::Walk for {name} {{
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,_:&str) {{
        match self {{
{arms}
        }}
    }}
}}
'''


def no_heap(names):
    return ''.join(implement(name, []) for name in names)


def patches(stage):
    additions = dict(BRIDGES)
    def read(path):
        return (stage/'crates'/path).read_text()
    def add(path, text):
        additions[path] = additions.get(path, '') + text
    add('tsr_jsstring/src/lib.rs', '\npub mod census;\n')
    # These use crate:: because census itself lives in this lowest dependency.
    for path, name, selected in [
        ('jsstring.rs','JsString',['storage']),
        ('source_text.rs','SourceText',['0']),
        ('position_map.rs','PositionMap',['entries']),
        ('position_map.rs','PositionMapEntry',[]),
    ]:
        add('tsr_jsstring/src/'+path, implement(name,selected).replace('tsr_jsstring::','crate::'))
    add('tsr_core/src/pattern.rs', implement('Pattern',['text']))
    add('tsr_arena/src/ids.rs', no_heap(['NodeId','AuxId','SymbolId','ArenaId','FileId']))

    # Generated payloads have an exhaustive enum match and a checked field-type
    # vocabulary. Unknown field types fail generation instead of disappearing.
    data = read('tsr_ast/src/data_generated.rs')
    simple = {'JsString','NodeKind','NodeSlice','TextSlice','Option<NodeId>',
              'Option<NodeListId>','bool','i32','DeferredField'}
    for name in re.findall(r'^pub struct (\w+)\s*\{',data,re.M):
        fs=fields(data,name)
        for field,ty in fs:
            if ty not in simple:
                raise ValueError(f'unclassified generated field {name}.{field}: {ty}')
        add('tsr_ast/src/data_generated.rs',implement(name,[f for f,t in fs if t=='JsString']))
    add('tsr_ast/src/data_generated.rs',enum_walk(data,'NodeData'))
    add('tsr_ast/src/lib.rs', implement('Node',['data']) + no_heap(['DeferredField','NodeKind']))
    add('tsr_ast/src/storage.rs', implement('AstFile',['0']))
    add('tsr_ast/src/lists.rs', enum_walk(read('tsr_ast/src/lists.rs'),'AstStorageData') +
        no_heap(['NodeList','NodeListId','NodeSlice','TextSlice','FileInfo']))
    add('tsr_ast/src/metadata.rs',enum_walk(read('tsr_ast/src/metadata.rs'),'SourceMetadataData'))
    add('tsr_ast/src/tokens.rs',no_heap(['CommentDirective']))
    source_types = {
        'SourceFileParseOptions':['file_name','path'],
        'FileReference':['file_name'], 'PragmaArgument':['name','value'],
        'Pragma':['name','args'], 'MappedDiagnosticDirective':['unused_message_text','source'],
        'SpanSegment':[],
        'ContentMapperSourceFileInfo':['content_mapper','transform_identity','parse_options',
            'virtual_file_name','original_text','span_map'],
        'SourceFileState':['parse_options','text','reparsed_clones','diagnostics','js_diagnostics',
            'jsdoc_diagnostics','content_mapper_info','position_map','ecma_line_map','node_index','binding'],
    }
    source=read('tsr_ast/src/source_file.rs')
    for name, selected in source_types.items():
        known=dict(fields(source,name))
        if not set(selected)<=known.keys():
            raise ValueError(f'missing selected field in {name}')
        # Every explicitly heap-bearing field must be traversed. The other
        # source slice fields are non-owning AuxIds walked in allocated aux slots.
        for field,ty in known.items():
            if (any(word in ty for word in ('Vec<','Arc<','Map<','OnceLock<')) or ty in {'JsString','SourceText'}) and field not in selected:
                raise ValueError(f'unwalked owning source field {name}.{field}: {ty}')
        add('tsr_ast/src/source_file.rs',implement(name,selected))
    add('tsr_ast/src/source_cache.rs',implement('SourceNodeIndexCache',['value']))
    add('tsr_ast/src/node_index.rs',implement('NodeIndexCache',['nodes','sorted']))
    add('tsr_ast/src/diagnostic.rs',implement('Diagnostic',['source','message_text','message_key','message_args','message_chain','related_information']))
    add('tsr_ast/src/symbols.rs',implement('Symbol',['name']) + implement('SymbolTables',['0']) +
        implement('DeclarationLists',['0']) + no_heap(['SymbolTableId','DeclarationSlice']))
    add('tsr_ast/src/flow.rs',implement('FlowNodes',['0'])+implement('FlowLists',['0'])+
        no_heap(['FlowNode','FlowList','FlowId','FlowListId']))
    add('tsr_ast/src/bind_result.rs',implement('BindResult',['nodes','bindings','flow_bindings','symbols',
        'tables','declarations','flows','flow_lists','diagnostics','pattern_ambient_modules'])+
        implement('BindCell',['0'])+implement('PatternAmbientModule',['pattern'])+
        implement('BoundFile',['file'])+no_heap(['NodeBinding']))
    add('tsr_ast/src/lib.rs',r'''
/// Diagnostic stage-only ownership walk. Call after the native snapshot, with
/// all workers stopped and every retained root still alive.
pub fn retained_census(files: &[BoundFile]) -> tsr_jsstring::census::Report {
    let mut result=tsr_jsstring::census::Report::default();
    for file in files { add_retained_file(file,&mut result); }
    result
}
pub fn add_retained_file(file:&BoundFile,result:&mut tsr_jsstring::census::Report) {
    tsr_jsstring::census::Walk::walk(file,result,"retained.root");
}
''')
    result={}
    for relative, extra in additions.items():
        original=read(relative)
        if MARKER in original:
            raise ValueError(f'already instrumented: {relative}')
        qualifier='crate' if relative.startswith('tsr_jsstring/') else 'tsr_jsstring'
        trait_import=f'\n#[allow(unused_imports)]\nuse {qualifier}::census::Walk as _;\n'
        extra=extra.replace('        use tsr_jsstring::census::Walk;\n','')
        result['crates/'+relative]=original+'\n'+MARKER+trait_import+extra
    result['crates/tsr_jsstring/src/census.rs']=(HERE/'collector.rs').read_text()
    return result


def apply(stage):
    stage=stage.resolve()
    if stage==ROOT or (ROOT/'crates') in stage.parents or not (stage/'crates/tsr_ast/Cargo.toml').is_file():
        raise ValueError('requires an existing staged repository, never the production crates')
    changes=patches(stage); manifest=[]; patch=[]
    for relative,text in sorted(changes.items()):
        path=stage/relative; original_path=ROOT/relative
        if path.is_symlink() or (path.exists() and original_path.exists() and path.samefile(original_path)):
            raise ValueError(f'staged path aliases production: {relative}')
        if stage not in path.resolve().parents:
            raise ValueError(f'staged path escapes destination: {relative}')
        old=path.read_bytes() if path.exists() else b''; new=text.encode()
        manifest.append({'path':relative,'before_sha256':sha(old) if path.exists() else None,
                         'after_sha256':sha(new),'before_bytes':len(old),'after_bytes':len(new)})
        patch.extend(difflib.unified_diff(old.decode().splitlines(keepends=True),text.splitlines(keepends=True),
                      fromfile='a/'+relative if path.exists() else '/dev/null',tofile='b/'+relative))
    # All destination and generation checks complete before any mutation.
    for relative,text in sorted(changes.items()):
        path=stage/relative; path.parent.mkdir(parents=True,exist_ok=True); path.write_text(text)
    patch_bytes=''.join(patch).encode();(stage/'census.patch').write_bytes(patch_bytes)
    record={'schema':1,'stage':str(stage),'production_source':str(ROOT),'operation':'additive-stage-only-retained-census',
            'patch_sha256':sha(patch_bytes),'files':manifest,
            'generator_files':[{'name':p.name,'sha256':sha(p.read_bytes())} for p in sorted(HERE.glob('*.py'))]+[
                {'name':'collector.rs','sha256':sha((HERE/'collector.rs').read_bytes())}]}
    (stage/'census-manifest.json').write_text(json.dumps(record,indent=2)+'\n')
    return record


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__); parser.add_argument('--stage',required=True,type=Path)
    print(json.dumps(apply(parser.parse_args().stage),indent=2))

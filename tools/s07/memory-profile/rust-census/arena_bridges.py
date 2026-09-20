"""Stage-only implementations appended in the modules owning private fields."""

BOUNDS = 'N: NodeRecord + tsr_jsstring::census::Walk, N::Aux: tsr_jsstring::census::Walk, S: tsr_jsstring::census::Walk'

BRIDGES = {
'tsr_arena/src/arena.rs': r'''
impl<T: tsr_jsstring::census::Walk> tsr_jsstring::census::Walk for Arena<T> {
    fn walk(&self, c: &mut tsr_jsstring::census::Collector, category: &str) {
        use tsr_jsstring::census::Walk;
        c.vector(&self.pages, &format!("{category}.page_directory"));
        for page in &self.pages {
            page.values.walk(c, &format!("{category}.page_payload"));
            page._allocation.walk(c, "arena.tracking");
        }
        self.counters.walk(c, "arena.tracking");
    }
}
''',
'tsr_arena/src/owned.rs': r'''
impl<T: tsr_jsstring::census::Walk> tsr_jsstring::census::Walk for OwnedArena<T> {
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,k:&str) { self.0.walk(c,k); }
}
impl<T: tsr_jsstring::census::Walk> tsr_jsstring::census::Walk for SymbolArena<T> {
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,k:&str) { self.0.walk(c,k); }
}
''',
'tsr_arena/src/counters.rs': r'''
impl tsr_jsstring::census::Walk for State {
    fn walk(&self,_:&mut tsr_jsstring::census::Collector,_:&str) {}
}
impl tsr_jsstring::census::Walk for Counters {
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,k:&str) { self.0.walk(c,k); }
}
impl tsr_jsstring::census::Walk for Track {
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,k:&str) { self.counters.walk(c,k); }
}
''',
'tsr_arena/src/node_slots.rs': r'''
impl<T: tsr_jsstring::census::Walk> tsr_jsstring::census::Walk for NodeSlots<T> {
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,k:&str) {
        use tsr_jsstring::census::Walk;
        c.vector(&self.pages,&format!("{k}.page_directory"));
        for page in self.pages.iter().flatten() {
            let used=page.iter().filter(|v|v.is_some()).count();
            c.record(&format!("{k}.page_slots"),used*std::mem::size_of::<Option<T>>(),
                std::mem::size_of_val(&**page),used,PAGE,1);
            for value in page.iter().flatten() { value.walk(c,k); }
        }
        self.foreign.walk(c,&format!("{k}.foreign"));
    }
}
''',
'tsr_arena/src/lazy.rs': r'''
impl<T: tsr_jsstring::census::Walk> tsr_jsstring::census::Walk for Page<T> {
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,k:&str) {
        use tsr_jsstring::census::Walk;
        // Allocation payload includes every OnceLock, initialized or not.
        // The Arc walker has already charged that payload. Counts here expose
        // reserved/initialized slot mass without adding its bytes a second time.
        let initialized=self.slots.iter().filter(|slot|slot.get().is_some()).count();
        c.record(&format!("{k}.initialized_slots"),0,0,initialized,PAGE_SIZE,0);
        for value in self.slots.iter().filter_map(OnceLock::get) { value.walk(c,k); }
        self._allocation.walk(c,"arena.tracking");
    }
}
impl<T: tsr_jsstring::census::Walk> tsr_jsstring::census::Walk for Pages<T> {
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,k:&str) {
        use tsr_jsstring::census::Walk;
        c.vector(&self.directory,&format!("{k}.page_directory"));
        c.record(&format!("{k}.reserved_slots"),0,0,self.reserved,self.directory.len()*PAGE_SIZE,0);
        for page in &self.directory { page.walk(c,&format!("{k}.page_payload")); }
        self.counters.walk(c,"arena.tracking");
    }
}
impl tsr_jsstring::census::Walk for TokenKey { fn walk(&self,_:&mut tsr_jsstring::census::Collector,_:&str) {} }
impl tsr_jsstring::census::Walk for CachedToken { fn walk(&self,_:&mut tsr_jsstring::census::Collector,_:&str) {} }
impl<N> tsr_jsstring::census::Walk for LazyArena<N>
where N:NodeRecord+tsr_jsstring::census::Walk,N::Aux:tsr_jsstring::census::Walk {
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,_:&str) {
        use tsr_jsstring::census::Walk;
        let state=self.read();
        state.pages.walk(c,"lazy.nodes"); state.auxiliary.walk(c,"lazy.auxiliary");
        state.jsdoc.walk(c,"lazy.jsdoc_cache"); state.tokens.walk(c,"lazy.token_cache");
    }
}
''',
'tsr_arena/src/file.rs': '''
impl<N,S> tsr_jsstring::census::Walk for StorageOwner<N,S>
where ''' + BOUNDS + r''' {
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,_:&str) {
        use tsr_jsstring::census::Walk;
        self.core.walk(c,"core.nodes"); self.auxiliary.walk(c,"core.auxiliary");
        self.symbols.walk(c,"core.symbols"); self.lazy.walk(c,"lazy");
        self.source.walk(c,"source.bytes"); self.position_map.walk(c,"source.position_map");
        self.supplemental.walk(c,"owner.supplemental");
        c.vector(&self.imports,"owner.import_directory");
        for import in &self.imports {
            let bytes=std::mem::size_of_val(&**import);
            c.record("owner.import_handle",bytes,bytes,1,1,usize::from(bytes>0));
            import.borrowed_handle().walk(c,"owner.imported");
        }
        self.imported_arenas.walk(c,"owner.imported_arenas"); self._owner.walk(c,"arena.tracking");
    }
}
''',
'tsr_arena/src/bundle.rs': '''
impl<N,S> tsr_jsstring::census::Walk for StorageBundle<N,S>
where ''' + BOUNDS + r''' {
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,_:&str) {
        use tsr_jsstring::census::Walk;
        self.files.walk(c,"owner.bundle_directory"); self._owner.walk(c,"arena.tracking");
    }
}
impl<N,S> tsr_jsstring::census::Walk for StorageHandle<N,S>
where ''' + BOUNDS + r''' {
    fn walk(&self,c:&mut tsr_jsstring::census::Collector,_:&str) {
        use tsr_jsstring::census::Walk;
        match &self.root {
            Root::File(file)=>file.walk(c,"owner.file"),
            Root::Bundle(bundle,_)=>bundle.walk(c,"owner.bundle"),
        }
    }
}
''',
}

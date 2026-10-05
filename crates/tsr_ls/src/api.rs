//! Direct API queries retain their checker owner before releasing the caller's
//! query lease. Positions use the compiler's byte offsets, not LSP characters.
use crate::{syntax::Syntax, Error, LanguageService, QueryChecker, Result};
use tsr_ast::NodeId;
use tsr_checker::{Operation, RetainedSymbol, RetainedType};

impl LanguageService<'_> {
    /// The caller selects a checker through its ordinary query scheduler. The
    /// returned symbol retains that exact checker's interpretation and storage.
    /// port: tsc/internal/ls/api.go:LanguageService.GetSymbolAtPosition
    pub fn get_symbol_at_position(
        &self,
        checkers: &mut impl QueryChecker,
        file_name: &[u8],
        byte_position: i64,
    ) -> Result<Option<RetainedSymbol>> {
        self.check_canceled()?;
        let file = self.program.source_file(file_name).ok_or_else(|| {
            Error::MissingSourceFile(String::from_utf8_lossy(file_name).into_owned())
        })?;
        let mut syntax = Syntax::new(file.bound().view().ast(), file.source())?;
        // The pinned navigator returns an enclosing node even outside a token;
        // its Rust contract therefore cannot yield Go's defensive nil-token case.
        let node = syntax.nav().get_token_at_position(byte_position)?;
        self.get_symbol_at_location(checkers, node)
    }

    /// A raw node identity is checked against this service's program before the
    /// provider is invoked. A foreign or retired checker is rejected by its
    /// operation before reading the node.
    /// port: tsc/internal/ls/api.go:LanguageService.GetSymbolAtLocation
    pub fn get_symbol_at_location(
        &self,
        checkers: &mut impl QueryChecker,
        node: NodeId,
    ) -> Result<Option<RetainedSymbol>> {
        self.check_canceled()?;
        let file = self
            .program
            .file_of_node(node)
            .ok_or(tsr_arena::Error::WrongOwner)?;
        file.bound().view().ast().node(node)?;
        checkers.with_checker(file.source(), |checker| {
            checker
                .get_symbol_at_location(node)?
                .map(|symbol| checker.retain_symbol_ref(symbol))
                .transpose()
                .map_err(Into::into)
        })
    }

    /// The caller acquires an operation of `symbol.owner()` before calling.
    /// This explicit acquisition avoids reentry on an already-held checker;
    /// another checker's equal numeric symbol identity cannot be imported.
    /// port: tsc/internal/ls/api.go:LanguageService.GetTypeOfSymbol
    pub fn get_type_of_symbol(
        &self,
        checker: &mut Operation<'_>,
        symbol: &RetainedSymbol,
    ) -> Result<RetainedType> {
        self.check_canceled()?;
        let symbol = checker.import_symbol_ref(symbol)?;
        let ty = checker.get_type_of_symbol_at_location(symbol, None)?;
        Ok(checker.retain_type(ty)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tsr_core::CancellationToken;

    #[test]
    fn direct_api_retains_symbol_and_type_after_pool_and_program_drop() {
        let program = Arc::new(crate::tests::program(
            b"/index.ts",
            b"export const value = 123; value;",
        ));
        let weak_program = Arc::downgrade(&program);
        let pool =
            tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
        let service = LanguageService::new(
            &program,
            tsr_jsstring::PositionEncoding::Utf16,
            CancellationToken::new(),
        );
        let mut checker = pool
            .checker_for_file_exclusive(program.source_file(b"/index.ts").unwrap().source())
            .unwrap();
        let symbol = service
            .get_symbol_at_position(&mut checker, b"/index.ts", 13)
            .unwrap()
            .unwrap();
        let ty = service.get_type_of_symbol(&mut checker, &symbol).unwrap();
        let imported = checker.import_type(&ty).unwrap();
        assert!(
            checker.type_flags(imported).unwrap() & tsr_checker::type_flags::NUMBER_LITERAL != 0
        );
        drop(checker);
        drop(service);
        drop(pool);
        drop(program);
        assert!(weak_program.upgrade().is_some());
        let checker = symbol.owner().operation().unwrap();
        let imported = checker.import_symbol_ref(&symbol).unwrap();
        assert_eq!(checker.symbol(imported).unwrap().name_bytes(), b"value");
        drop(checker);
        drop(symbol);
        assert!(weak_program.upgrade().is_some());
        drop(ty);
        assert!(weak_program.upgrade().is_none());
    }

    #[test]
    fn direct_api_validates_missing_file_foreign_symbol_and_cancellation() {
        let program = Arc::new(crate::tests::program(b"/index.ts", b"const value = 1;"));
        let pool =
            tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
        let other_pool =
            tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
        let token = CancellationToken::new();
        let service = LanguageService::new(
            &program,
            tsr_jsstring::PositionEncoding::Utf8,
            token.clone(),
        );
        let source = program.source_file(b"/index.ts").unwrap().source();
        let mut checker = pool.checker_for_file_exclusive(source).unwrap();
        let error = service
            .get_symbol_at_position(&mut checker, b"/missing.ts", 0)
            .unwrap_err();
        assert_eq!(error.to_string(), "source file not found: /missing.ts");
        let symbol = service
            .get_symbol_at_position(&mut checker, b"/index.ts", 6)
            .unwrap()
            .unwrap();
        let mut other = other_pool.checker_for_file_exclusive(source).unwrap();
        assert!(service.get_type_of_symbol(&mut other, &symbol).is_err());
        token.cancel();
        assert!(matches!(
            service.get_type_of_symbol(&mut checker, &symbol),
            Err(Error::Canceled)
        ));
    }
}

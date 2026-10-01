/// A file whose positions the printer maps: the pinned `sourcemap.Source`.
pub trait Source {
    fn text(&self) -> &[u8];
    fn file_name(&self) -> &[u8];
    /// The ECMAScript line starts, as byte offsets.
    fn ecma_line_map(&self) -> &[i32];
}

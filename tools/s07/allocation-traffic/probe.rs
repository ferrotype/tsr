#[global_allocator]
static ALLOCATOR: cap::Cap<mimalloc::MiMalloc> = cap::Cap::new(mimalloc::MiMalloc, usize::MAX);
fn main() {
    tsr_ast::calibrate_allocation_traffic(|| [ALLOCATOR.total_allocated(), ALLOCATOR.allocated()]);
    tsr_parser::calibrate_parser_list_traffic(|| [ALLOCATOR.total_allocated(), ALLOCATOR.allocated()]);
    println!("page/directory live/drop, shrink, text pool reuse/drop, eager parser spill/abort/transfer/append and lazy exclusion calibration passed");
}

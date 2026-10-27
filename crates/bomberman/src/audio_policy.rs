//! Native effect-admission rule at BM95.EXE 0x427859.
//! VALUELST8=5 is inclusive: the original admits a sixth concurrent sample.
pub fn permits_effect(active_one_shots: usize, resource_limit: i32) -> bool {
    resource_limit >= 0 && active_one_shots <= resource_limit as usize
}

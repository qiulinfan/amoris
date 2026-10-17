// A real dispatch keeps Metal from omitting the clock calibration's pass timestamps.
@group(0) @binding(0) var<storage, read_write> clock_work: u32;

@compute @workgroup_size(1)
fn main() {
    clock_work += 1u;
}
